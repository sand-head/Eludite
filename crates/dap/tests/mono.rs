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
use eludite_dap::types::{Event, SourceBreakpoint};
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

#[test]
fn eludite_dbg_mono_debugs_the_test_app() {
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
            return;
        }
        Err(e) => {
            eprintln!("skipped: {e}");
            return;
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
        return;
    };
    let app_dir = mono_dir.join("Eludite.Debugger.Mono.TestApp");
    if built(&app_dir, "Eludite.Debugger.Mono.TestApp.exe").is_none() {
        eprintln!("skipped: the TestApp is not built (dotnet build dotnet/Eludite.slnx)");
        return;
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
    let line_of = |mark: &str| {
        text.lines()
            .position(|l| l.ends_with(&format!("// MARK: {mark}")))
            .unwrap_or_else(|| panic!("MARK: {mark}")) as i64
            + 1
    };
    let version = mono.version().expect("mono --version");
    eprintln!(
        "eludite-dbg-mono under mono {version} ({})",
        mono.mono.display()
    );

    let rec = Recorder::default();
    let clock = Instant::now();
    let client = DapClient::start(
        transport::connect_with_env(&mono.adapter_transport(&adapter), &mono.env).unwrap(),
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
