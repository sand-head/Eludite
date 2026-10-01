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
