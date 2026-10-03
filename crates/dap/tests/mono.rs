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

/// What the tests need: Mono, the adapter, the TestApp's launch configuration and its `Program.cs`; `None` (with a
/// message) to skip.
struct Found {
    mono: eludite_dap::discovery::MonoInstall,
    adapter: PathBuf,
    config: launch::LaunchConfig,
    source: PathBuf,
    text: String,
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
        let rec = Recorder::default();
        let client = DapClient::start(
            transport::connect_with_env(
                &self.mono.adapter_transport(&self.adapter),
                &self.mono.env,
            )
            .unwrap(),
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
                breakpoints: vec![(
                    self.source.to_string_lossy().into_owned(),
                    marks
                        .iter()
                        .map(|m| SourceBreakpoint {
                            line: self.line_of(m),
                            ..Default::default()
                        })
                        .collect(),
                )],
                function_breakpoints: Vec::new(),
                exception_filters: filters.iter().map(|f| (*f).to_owned()).collect(),
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
        transport::connect_with_env(&mono.adapter_transport(adapter), &mono.env).unwrap(),
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
            function_breakpoints: Vec::new(),
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
