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
    let client = DapClient::start(transport::connect(&found.transport()).unwrap(), rec.sink());
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
    let client = DapClient::start(transport::connect(&found.transport()).unwrap(), rec.sink());
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
