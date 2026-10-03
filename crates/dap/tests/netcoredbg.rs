//! The client against the real netcoredbg debugging the real `eludite-host` (Linux): launch, a breakpoint in
//! `HostRpcTarget.Ping` hit by an `eludite/ping` written to the debuggee's stdin, the stack, locals, evaluate, a
//! step, continue and stop. Skipped unless netcoredbg is found (`ELUDITE_NETCOREDBG`, `PATH`; see
//! `tools/netcoredbg/fetch.sh`) and `dotnet build dotnet/Eludite.slnx` has run.

mod common;

#[cfg(target_os = "linux")]
#[test]
fn netcoredbg_debugs_eludite_host() {
    use std::io::Write;
    use std::path::PathBuf;

    use eludite_dap::discovery::AdapterSearch;
    use eludite_dap::launch;
    use eludite_dap::session::{self, StartKind, StartPlan};
    use eludite_dap::types::{Event, SourceBreakpoint};
    use eludite_dap::{ClientEvent, DapClient, transport};
    use serde_json::json;

    use common::{Recorder, T};

    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let project = root.join("dotnet/src/Eludite.Host/Eludite.Host.csproj");
    let source = root.join("dotnet/src/Eludite.Host/Rpc/HostRpcTarget.cs");
    let found = match AdapterSearch::from_env().find_netcoredbg() {
        Ok(f) => f,
        Err(e) => {
            eprintln!("skipped: {e}");
            return;
        }
    };
    let config = match launch::launch_config(&project, None) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("skipped: {e}");
            return;
        }
    };
    let text = std::fs::read_to_string(&source).unwrap();
    // The first statement of Ping.
    let line = text
        .lines()
        .position(|l| l.contains("var timestamp = _timeProvider"))
        .expect("Ping's first statement") as i64
        + 1;
    let source = std::fs::canonicalize(&source).unwrap();
    let rec = Recorder::default();
    let client = DapClient::start(
        common::recorded(
            transport::connect(&found.transport()).unwrap(),
            "netcoredbg",
            "netcoredbg",
            None,
        ),
        rec.sink(),
    );
    let mut args = config.netcoredbg_arguments();
    args["args"] = json!(["--stdio", "--no-roslyn"]);
    let started = session::start(
        &client,
        &StartPlan {
            adapter_id: "coreclr".into(),
            kind: StartKind::Launch,
            arguments: args,
            breakpoints: vec![(
                source.to_string_lossy().into_owned(),
                vec![SourceBreakpoint {
                    line,
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
    let pid = match rec.wait_nth(1, "process", |e| {
        matches!(e, ClientEvent::Event(Event::Process(_)))
    }) {
        ClientEvent::Event(Event::Process(p)) => p.system_process_id.unwrap(),
        _ => unreachable!(),
    };
    rec.wait_nth(
        1,
        "breakpoint bound",
        |e| matches!(e, ClientEvent::Event(Event::Breakpoint(b)) if b.breakpoint.verified),
    );
    // The host reads JSON-RPC on stdin, which netcoredbg holds as a pipe: write the ping into it.
    let ping = br#"{"jsonrpc":"2.0","id":1,"method":"eludite/ping"}"#;
    let mut stdin = std::fs::OpenOptions::new()
        .write(true)
        .open(format!("/proc/{pid}/fd/0"))
        .unwrap();
    write!(stdin, "Content-Length: {}\r\n\r\n", ping.len()).unwrap();
    stdin.write_all(ping).unwrap();
    drop(stdin);
    let s = rec.stopped(1);
    assert_eq!(s.reason, "breakpoint");
    let tid = s.thread_id.unwrap();
    let st = client
        .request_wait("stackTrace", json!({"threadId": tid, "levels": 20}), T)
        .unwrap();
    assert_eq!(
        st["stackFrames"][0]["name"],
        "Eludite.Host.Rpc.HostRpcTarget.Ping()"
    );
    assert_eq!(st["stackFrames"][0]["line"], line);
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
    let names: Vec<&str> = vars["variables"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|v| v["name"].as_str())
        .collect();
    assert_eq!(names, ["this", "timestamp"]);
    let e = client
        .request_wait(
            "evaluate",
            json!({"expression": "_timeProvider", "frameId": frame, "context": "hover"}),
            T,
        )
        .unwrap();
    assert!(
        e["result"].as_str().unwrap().contains("TimeProvider"),
        "{e}"
    );
    client
        .request_wait("next", json!({"threadId": tid}), T)
        .unwrap();
    assert_eq!(rec.stopped(2).reason, "step");
    client
        .request_wait("continue", json!({"threadId": tid}), T)
        .unwrap();
    rec.wait_nth(
        1,
        "pong",
        |e| matches!(e, ClientEvent::Event(Event::Output(o)) if o.output.contains("\"pong\":true")),
    );
    client
        .request_wait("disconnect", json!({"terminateDebuggee": true}), T)
        .unwrap();
    rec.wait_nth(1, "terminated", |e| {
        matches!(e, ClientEvent::Event(Event::Terminated))
    });
}

/// Brief 0025's requests against the real netcoredbg debugging `eludite-host` (Linux): `pause` of the host idling on
/// stdin, `stackTrace` paging of the paused thread (with `startFrame` and `levels` when netcoredbg advertises
/// delayed loading, else checking it answers the whole stack), every thread's top frame, then `exceptionInfo` at the
/// first-chance `LocalRpcException` that `eludite/solution/close` throws before `eludite/host/initialize`, and
/// `variables` with `start` and `count` on that frame's `this`. Skipped like the test above; it prints what the
/// adapter does with the paging arguments (brief 0025's adapter matrix).
#[cfg(target_os = "linux")]
#[test]
fn netcoredbg_pauses_pages_and_explains_exceptions() {
    use std::io::Write;
    use std::path::PathBuf;
    use std::time::{Duration, Instant};

    use eludite_dap::discovery::AdapterSearch;
    use eludite_dap::launch;
    use eludite_dap::session::{self, StartKind, StartPlan};
    use eludite_dap::types::Event;
    use eludite_dap::{ClientEvent, DapClient, transport};
    use serde_json::json;

    use common::{Recorder, T};

    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let project = root.join("dotnet/src/Eludite.Host/Eludite.Host.csproj");
    let found = match AdapterSearch::from_env().find_netcoredbg() {
        Ok(f) => f,
        Err(e) => {
            eprintln!("skipped: {e}");
            return;
        }
    };
    let config = match launch::launch_config(&project, None) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("skipped: {e}");
            return;
        }
    };
    let rec = Recorder::default();
    let client = DapClient::start(
        common::recorded(
            transport::connect(&found.transport()).unwrap(),
            "netcoredbg",
            "netcoredbg",
            None,
        ),
        rec.sink(),
    );
    let mut args = config.netcoredbg_arguments();
    args["args"] = json!(["--stdio", "--no-roslyn"]);
    let started = session::start(
        &client,
        &StartPlan {
            adapter_id: "coreclr".into(),
            kind: StartKind::Launch,
            arguments: args,
            breakpoints: Vec::new(),
            exception_filters: Vec::new(),
            exception_options: Vec::new(),
            function_breakpoints: Vec::new(),
        },
        T,
    )
    .unwrap();
    let caps = started.capabilities;
    eprintln!(
        "netcoredbg capabilities: delayed stack loading {}, variable paging {}, exceptionInfo {}",
        caps.supports_delayed_stack_trace_loading,
        caps.supports_variable_paging,
        caps.supports_exception_info_request
    );
    let pid = match rec.wait_nth(1, "process", |e| {
        matches!(e, ClientEvent::Event(Event::Process(_)))
    }) {
        ClientEvent::Event(Event::Process(p)) => p.system_process_id.unwrap(),
        _ => unreachable!(),
    };
    // Let the host reach its read of stdin, then Break All.
    std::thread::sleep(Duration::from_millis(1500));
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
    let threads = client.request_wait("threads", json!({}), T).unwrap();
    let ids: Vec<i64> = threads["threads"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["id"].as_i64().unwrap())
        .collect();
    assert!(!ids.is_empty());
    let tid = s.thread_id.unwrap_or(ids[0]);
    let all = client
        .request_wait("stackTrace", json!({"threadId": tid}), T)
        .unwrap();
    let n = all["stackFrames"].as_array().unwrap().len();
    let page = client
        .request_wait(
            "stackTrace",
            json!({"threadId": tid, "startFrame": 1, "levels": 2}),
            T,
        )
        .unwrap();
    let m = page["stackFrames"].as_array().unwrap().len();
    eprintln!(
        "stackTrace: {n} frames in all, totalFrames {:?}; startFrame 1 levels 2 answered {m}",
        all.get("totalFrames")
    );
    if caps.supports_delayed_stack_trace_loading {
        assert_eq!(m, 2.min(n.saturating_sub(1)));
        assert_eq!(
            page["stackFrames"][0]["name"],
            all["stackFrames"][1]["name"]
        );
    } else {
        assert!(m == n || m == 2.min(n.saturating_sub(1)), "{m} of {n}");
    }
    for t in &ids {
        let top = client
            .request_wait("stackTrace", json!({"threadId": t, "levels": 1}), T)
            .unwrap();
        assert!(top["stackFrames"].is_array());
    }
    // First-chance exceptions from now on, then a request the host refuses in its own code.
    client
        .request_wait("setExceptionBreakpoints", json!({"filters": ["all"]}), T)
        .unwrap();
    client
        .request_wait("continue", json!({"threadId": tid}), T)
        .unwrap();
    let close = br#"{"jsonrpc":"2.0","id":1,"method":"eludite/solution/close"}"#;
    let mut stdin = std::fs::OpenOptions::new()
        .write(true)
        .open(format!("/proc/{pid}/fd/0"))
        .unwrap();
    write!(stdin, "Content-Length: {}\r\n\r\n", close.len()).unwrap();
    stdin.write_all(close).unwrap();
    drop(stdin);
    let s = rec.stopped(2);
    assert_eq!(s.reason, "exception");
    let tid = s.thread_id.unwrap();
    let info = client
        .request_wait("exceptionInfo", json!({"threadId": tid}), T)
        .unwrap();
    eprintln!("exceptionInfo: {info}");
    assert!(
        info["exceptionId"]
            .as_str()
            .unwrap()
            .contains("LocalRpcException"),
        "{info}"
    );
    assert!(
        info["description"]
            .as_str()
            .unwrap_or_default()
            .contains("initialize"),
        "{info}"
    );
    let st = client
        .request_wait("stackTrace", json!({"threadId": tid, "levels": 1}), T)
        .unwrap();
    let scopes = client
        .request_wait("scopes", json!({"frameId": st["stackFrames"][0]["id"]}), T)
        .unwrap();
    let vars = client
        .request_wait(
            "variables",
            json!({"variablesReference": scopes["scopes"][0]["variablesReference"]}),
            T,
        )
        .unwrap();
    let this = vars["variables"]
        .as_array()
        .unwrap()
        .iter()
        .find(|v| v["name"] == "this")
        .cloned()
        .expect("this");
    let members = client
        .request_wait(
            "variables",
            json!({"variablesReference": this["variablesReference"]}),
            T,
        )
        .unwrap();
    let paged = client
        .request_wait(
            "variables",
            json!({"variablesReference": this["variablesReference"], "start": 1, "count": 2}),
            T,
        )
        .unwrap();
    let (all_n, page_n) = (
        members["variables"].as_array().unwrap().len(),
        paged["variables"].as_array().unwrap().len(),
    );
    eprintln!(
        "variables: `this` has {all_n} members (namedVariables {:?}); start 1 count 2 answered {page_n}",
        this.get("namedVariables")
    );
    assert!(page_n == 2.min(all_n.saturating_sub(1)) || page_n == all_n);
    client
        .request_wait("disconnect", json!({"terminateDebuggee": true}), T)
        .unwrap();
}

/// Brief 0026's requests against the real netcoredbg (Linux), skipped like the tests above. On `eludite-host`:
/// `setFunctionBreakpoints` on `HostRpcTarget.Ping` stops when a ping arrives, `setVariable` on its `timestamp` (20
/// timed calls: the 150 ms p95 budget), and `filterOptions` naming `StreamJsonRpc.LocalRpcException` stop at the throw
/// of `eludite/solution/close`. Then the emulated tracepoint's overhead (proposal 0001 risk 3): a console program with a
/// 200-iteration loop is written to a temporary folder and built with `dotnet build`, and a breakpoint on its body is
/// handled as the shell handles a tracepoint where the adapter has no log points (stack, `evaluate`, `continue`).
#[cfg(target_os = "linux")]
#[test]
fn netcoredbg_runs_under_control() {
    use std::io::Write;
    use std::path::PathBuf;
    use std::time::{Duration, Instant};

    use eludite_dap::discovery::AdapterSearch;
    use eludite_dap::launch;
    use eludite_dap::session::{self, StartKind, StartPlan};
    use eludite_dap::types::{Event, ExceptionFilterOptions, FunctionBreakpoint, SourceBreakpoint};
    use eludite_dap::{ClientEvent, DapClient, transport};
    use serde_json::json;

    use common::{Recorder, T};

    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let project = root.join("dotnet/src/Eludite.Host/Eludite.Host.csproj");
    let found = match AdapterSearch::from_env().find_netcoredbg() {
        Ok(f) => f,
        Err(e) => {
            eprintln!("skipped: {e}");
            return;
        }
    };
    let config = match launch::launch_config(&project, None) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("skipped: {e}");
            return;
        }
    };
    let rec = Recorder::default();
    let client = DapClient::start(
        common::recorded(
            transport::connect(&found.transport()).unwrap(),
            "netcoredbg",
            "netcoredbg",
            None,
        ),
        rec.sink(),
    );
    let mut args = config.netcoredbg_arguments();
    args["args"] = json!(["--stdio", "--no-roslyn"]);
    let started = session::start(
        &client,
        &StartPlan {
            adapter_id: "coreclr".into(),
            kind: StartKind::Launch,
            arguments: args,
            breakpoints: Vec::new(),
            exception_filters: Vec::new(),
            exception_options: vec![ExceptionFilterOptions {
                filter_id: "all".into(),
                condition: Some("StreamJsonRpc.LocalRpcException".into()),
            }],
            function_breakpoints: vec![FunctionBreakpoint {
                name: "Eludite.Host.Rpc.HostRpcTarget.Ping".into(),
                ..Default::default()
            }],
        },
        T,
    )
    .unwrap();
    eprintln!(
        "netcoredbg: function breakpoint answer {:?}; setExpression {}; log points {}",
        started.function_breakpoints,
        started.capabilities.supports_set_expression,
        started.capabilities.supports_log_points
    );
    assert!(started.capabilities.supports_exception_filter_options);
    let pid = match rec.wait_nth(1, "process", |e| {
        matches!(e, ClientEvent::Event(Event::Process(_)))
    }) {
        ClientEvent::Event(Event::Process(p)) => p.system_process_id.unwrap(),
        _ => unreachable!(),
    };
    let write = |body: &[u8]| {
        let mut stdin = std::fs::OpenOptions::new()
            .write(true)
            .open(format!("/proc/{pid}/fd/0"))
            .unwrap();
        write!(stdin, "Content-Length: {}\r\n\r\n", body.len()).unwrap();
        stdin.write_all(body).unwrap();
    };
    std::thread::sleep(Duration::from_millis(1500));
    write(br#"{"jsonrpc":"2.0","id":1,"method":"eludite/ping"}"#);
    let s = rec.stopped(1);
    eprintln!("function breakpoint stop: reason {}", s.reason);
    assert!(s.reason.contains("breakpoint"), "{s:?}");
    let tid = s.thread_id.unwrap();
    let st = client
        .request_wait("stackTrace", json!({"threadId": tid, "levels": 1}), T)
        .unwrap();
    assert!(
        st["stackFrames"][0]["name"]
            .as_str()
            .unwrap()
            .contains("Ping"),
        "{st}"
    );
    let scopes = client
        .request_wait("scopes", json!({"frameId": st["stackFrames"][0]["id"]}), T)
        .unwrap();
    let locals = scopes["scopes"][0]["variablesReference"].clone();
    let mut times = Vec::new();
    for n in 0..20 {
        let clock = Instant::now();
        let r = client.request_wait(
            "setVariable",
            json!({"variablesReference": locals, "name": "timestamp", "value": format!("\"t{n}\"")}),
            T,
        );
        times.push(clock.elapsed());
        assert!(r.is_ok(), "{r:?}");
    }
    let (mean, p95) = common::mean_p95(&times);
    eprintln!(
        "timing: setVariable round trip against netcoredbg: mean {mean:.2} ms, p95 {p95:.2} ms (20 calls)"
    );
    assert!(p95 < 150.0, "{p95}");
    client
        .request_wait("continue", json!({"threadId": tid}), T)
        .unwrap();
    write(br#"{"jsonrpc":"2.0","id":2,"method":"eludite/solution/close"}"#);
    let s = rec.stopped(2);
    assert_eq!(s.reason, "exception");
    client
        .request_wait("disconnect", json!({"terminateDebuggee": true}), T)
        .unwrap();

    // The emulated tracepoint's overhead over 200 hits of a hot loop.
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("Loop.csproj"),
        "<Project Sdk=\"Microsoft.NET.Sdk\"><PropertyGroup><OutputType>Exe</OutputType>\
         <TargetFramework>net10.0</TargetFramework></PropertyGroup></Project>",
    )
    .unwrap();
    let program = "var sum = 0L;\nfor (var i = 0; i < 200; i++)\n{\n    sum += i;\n}\nSystem.Console.WriteLine(\"done \" + sum);\n";
    std::fs::write(dir.path().join("Program.cs"), program).unwrap();
    let built = std::process::Command::new("dotnet")
        .arg("build")
        .arg(dir.path().join("Loop.csproj"))
        .output();
    match built {
        Ok(o) if o.status.success() => {}
        other => {
            eprintln!(
                "skipped the overhead measurement: dotnet build of the loop failed: {other:?}"
            );
            return;
        }
    }
    let config = launch::launch_config(&dir.path().join("Loop.csproj"), None).unwrap();
    let rec = Recorder::default();
    let client = DapClient::start(
        common::recorded(
            transport::connect(&found.transport()).unwrap(),
            "netcoredbg",
            "netcoredbg",
            None,
        ),
        rec.sink(),
    );
    let source = std::fs::canonicalize(dir.path().join("Program.cs")).unwrap();
    let clock = Instant::now();
    session::start(
        &client,
        &StartPlan {
            adapter_id: "coreclr".into(),
            kind: StartKind::Launch,
            arguments: config.netcoredbg_arguments(),
            breakpoints: vec![(
                source.to_string_lossy().into_owned(),
                vec![SourceBreakpoint {
                    line: 4,
                    ..Default::default()
                }],
            )],
            exception_filters: Vec::new(),
            exception_options: Vec::new(),
            function_breakpoints: Vec::new(),
        },
        T,
    )
    .unwrap();
    let hits = common::emulate_tracepoint(&client, &rec, 200, "i");
    let total = clock.elapsed();
    assert_eq!(hits[199].0, "199");
    let (mean, p95) = common::mean_p95(&hits.iter().map(|(_, t)| *t).collect::<Vec<_>>());
    eprintln!(
        "timing: an emulated tracepoint against netcoredbg, stop to resume: mean {mean:.2} ms, p95 {p95:.2} ms over \
         200 hits; launch to the last hit {:.0} ms",
        total.as_secs_f64() * 1e3
    );
    assert!(mean < 15.0, "the budget is 15 ms per hit: {mean}");
    rec.wait_nth(1, "terminated", |e| {
        matches!(e, ClientEvent::Event(Event::Terminated))
    });
    let _ = client.request_wait("disconnect", json!({"terminateDebuggee": true}), T);
}

/// Brief 0027: netcoredbg attaches by process id (the attach plan for runtime `dotnet`) to `eludite-host` started with
/// `dotnet`, a breakpoint in `HostRpcTarget.Ping` stops it when a ping arrives on its stdin, and `disconnect` without
/// terminating detaches: the host keeps running. Skipped unless netcoredbg and the host's build are found.
#[cfg(target_os = "linux")]
#[test]
fn netcoredbg_attaches_to_a_dotnet_process_and_detaches() {
    use std::io::Write;
    use std::path::PathBuf;
    use std::time::Instant;

    use eludite_dap::attach::{AttachAdapter, attach_plan};
    use eludite_dap::discovery::AdapterSearch;
    use eludite_dap::launch::{self, Platform};
    use eludite_dap::processes;
    use eludite_dap::session::{self, StartKind, StartPlan};
    use eludite_dap::types::SourceBreakpoint;
    use eludite_dap::{DapClient, transport};
    use serde_json::json;

    use common::{Recorder, T};

    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let project = root.join("dotnet/src/Eludite.Host/Eludite.Host.csproj");
    let source = root.join("dotnet/src/Eludite.Host/Rpc/HostRpcTarget.cs");
    let found = match AdapterSearch::from_env().find_netcoredbg() {
        Ok(f) => f,
        Err(e) => {
            eprintln!("skipped: {e}");
            return;
        }
    };
    let config = match launch::launch_config(&project, None) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("skipped: {e}");
            return;
        }
    };
    let mut host = std::process::Command::new("dotnet")
        .arg(&config.program)
        .args(["--stdio", "--no-roslyn"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .expect("dotnet");
    let listed = processes::list()
        .unwrap()
        .into_iter()
        .find(|p| p.pid == host.id())
        .unwrap();
    assert_eq!(listed.runtime, processes::Runtime::Dotnet);
    let plan = attach_plan(
        AttachAdapter::for_runtime(listed.runtime).unwrap(),
        host.id(),
        None,
        Platform::Linux,
    )
    .unwrap();
    let text = std::fs::read_to_string(&source).unwrap();
    let line = text
        .lines()
        .position(|l| l.contains("var timestamp = _timeProvider"))
        .expect("Ping's first statement") as i64
        + 1;
    let source = std::fs::canonicalize(&source).unwrap();
    let rec = Recorder::default();
    let clock = Instant::now();
    let client = DapClient::start(
        common::recorded(
            transport::connect(&found.transport()).unwrap(),
            "netcoredbg",
            "netcoredbg",
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
                source.to_string_lossy().into_owned(),
                vec![SourceBreakpoint {
                    line,
                    ..Default::default()
                }],
            )],
            exception_filters: Vec::new(),
            exception_options: Vec::new(),
            function_breakpoints: Vec::new(),
        },
        T,
    )
    .unwrap();
    eprintln!(
        "timing: netcoredbg attach handshake: {:.0} ms",
        clock.elapsed().as_secs_f64() * 1e3
    );
    let stdin = host.stdin.as_mut().unwrap();
    let ping = r#"{"jsonrpc":"2.0","id":1,"method":"eludite/ping","params":{}}"#;
    write!(stdin, "Content-Length: {}\r\n\r\n{ping}", ping.len()).unwrap();
    stdin.flush().unwrap();
    assert_eq!(rec.stopped(1).reason, "breakpoint");
    client
        .request_wait("disconnect", json!({"terminateDebuggee": false}), T)
        .unwrap();
    client.kill();
    std::thread::sleep(std::time::Duration::from_millis(200));
    assert!(host.try_wait().unwrap().is_none(), "the host keeps running");
    let _ = host.kill();
    let _ = host.wait();
}
