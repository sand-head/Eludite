//! Brief 0027: the process listing (this process and the children it spawns, with their runtimes and which of them it
//! started), attach plans, and `attach` and `restart` against the scripted fake adapter through `DapClient`.

mod common;

#[cfg(target_os = "linux")]
use std::process::{Child, Command};
#[cfg(target_os = "linux")]
use std::time::{Duration, Instant};

use eludite_dap::attach::{AttachAdapter, attach_plan};
use eludite_dap::fake;
use eludite_dap::launch::Platform;
#[cfg(target_os = "linux")]
use eludite_dap::processes::{self, Runtime};
use eludite_dap::session::{self, StartKind, StartPlan};
use eludite_dap::types::{Event, SourceBreakpoint};
use eludite_dap::{ClientEvent, DapClient, DapError};
use serde_json::json;

use common::{PROGRAM, Recorder, T, program};

/// Kills its child when dropped.
#[cfg(target_os = "linux")]
struct Kill(Child);

#[cfg(target_os = "linux")]
impl Drop for Kill {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// A copy of `sleep` named `name` in a temporary folder.
#[cfg(unix)]
fn sleeper(dir: &std::path::Path, name: &str) -> std::path::PathBuf {
    let sleep = ["/bin/sleep", "/usr/bin/sleep"]
        .into_iter()
        .find(|p| std::path::Path::new(p).exists())
        .expect("sleep");
    let to = dir.join(name);
    std::fs::copy(sleep, &to).unwrap();
    to
}

#[cfg(target_os = "linux")]
#[test]
fn the_listing_has_this_process_and_its_children_with_their_runtimes() {
    use std::os::unix::fs::PermissionsExt as _;
    let dir = tempfile::tempdir().unwrap();
    // A stand-in for `dotnet App.dll`: a shell script named dotnet (its command line is `/bin/sh .../dotnet App.dll`).
    let dotnet = dir.path().join("dotnet");
    std::fs::write(&dotnet, "#!/bin/sh\nwhile true; do sleep 1; done\n").unwrap();
    std::fs::set_permissions(&dotnet, std::fs::Permissions::from_mode(0o755)).unwrap();
    let app = Kill(Command::new(&dotnet).arg("/s/bin/App.dll").spawn().unwrap());
    // A program run by "mono" with a debugger agent that listens: a shell whose argv[0] is `mono`.
    use std::os::unix::process::CommandExt as _;
    let agent = Kill(
        Command::new("/bin/sh")
            .arg0("/usr/local/bin/mono")
            .args([
                "-c",
                "sleep 30; :",
                "sh",
                "--debugger-agent=transport=dt_socket,server=y,address=127.0.0.1:55001,suspend=n",
                "TestApp.exe",
            ])
            .spawn()
            .unwrap(),
    );
    let plain = Kill(
        Command::new(sleeper(dir.path(), "plain-sleeper"))
            .arg("30")
            .spawn()
            .unwrap(),
    );
    let me = std::process::id();
    // Wait until the shell script has started its `sleep` child.
    let deadline = Instant::now() + T;
    let (all, took) = loop {
        let t = Instant::now();
        let all = processes::list().unwrap();
        let took = t.elapsed();
        if all.iter().any(|p| p.parent == Some(app.0.id())) {
            break (all, took);
        }
        assert!(
            Instant::now() < deadline,
            "the script's child never appeared"
        );
        std::thread::sleep(Duration::from_millis(20));
    };
    eprintln!(
        "processes: {} listed in {:.1} ms",
        all.len(),
        took.as_secs_f64() * 1e3
    );
    let find = |pid: u32| all.iter().find(|p| p.pid == pid).cloned().unwrap();
    let this = find(me);
    assert_eq!(this.runtime, Runtime::Native);
    assert!(!this.command_line().is_empty());
    let a = find(app.0.id());
    assert_eq!(a.runtime, Runtime::Dotnet, "{a:?}");
    assert_eq!(a.parent, Some(me));
    assert!(
        a.command_line().ends_with("/dotnet /s/bin/App.dll"),
        "{}",
        a.command_line()
    );
    let m = find(agent.0.id());
    assert_eq!(m.runtime, Runtime::Mono);
    assert_eq!(m.name, "mono");
    assert_eq!(
        processes::mono_agent(&m.argv),
        Some(("127.0.0.1".to_owned(), 55001))
    );
    let p = find(plain.0.id());
    assert_eq!(
        (p.runtime, p.name.as_str()),
        (Runtime::Native, "plain-sleeper")
    );
    // Launched by Eludite: the roots and their descendants (the script's `sleep`), nothing else.
    let grandchild = all
        .iter()
        .find(|p| p.parent == Some(app.0.id()))
        .unwrap()
        .pid;
    let set = processes::launched_set(&[app.0.id()], &all);
    assert!(set.contains(&app.0.id()) && set.contains(&grandchild));
    assert!(!set.contains(&me) && !set.contains(&plain.0.id()));
    assert!(processes::is_launched(grandchild, &[app.0.id()]));
    assert!(processes::is_launched(app.0.id(), &[app.0.id()]));
    assert!(!processes::is_launched(plain.0.id(), &[app.0.id()]));
    assert!(
        processes::is_launched(plain.0.id(), &[me]),
        "a child of a root"
    );
    assert_eq!(processes::cwd_of(me), std::env::current_dir().ok());
    assert!(processes::alive(plain.0.id()));
    // No kernel threads.
    assert!(all.iter().all(|p| p.pid != 2));
    drop(plain);
    let gone = all.iter().find(|p| p.name == "plain-sleeper").unwrap().pid;
    assert!(!processes::alive(gone));
}

fn plan(kind: StartKind, arguments: serde_json::Value) -> StartPlan {
    StartPlan {
        adapter_id: "coreclr".into(),
        kind,
        arguments,
        breakpoints: vec![(
            PROGRAM.into(),
            vec![SourceBreakpoint {
                line: 6,
                ..Default::default()
            }],
        )],
        exception_filters: vec![],
        exception_options: vec![],
        function_breakpoints: vec![],
    }
}

fn process_event(rec: &Recorder, n: usize) -> Option<i64> {
    match rec.wait_nth(n, "process", |e| {
        matches!(e, ClientEvent::Event(Event::Process(_)))
    }) {
        ClientEvent::Event(Event::Process(p)) => p.system_process_id,
        _ => unreachable!(),
    }
}

#[test]
fn attach_runs_the_handshake_and_disconnect_detaches() {
    let (conn, handle) = fake::connect(program());
    let rec = Recorder::default();
    let client = DapClient::start(conn, rec.sink());
    let p = attach_plan(AttachAdapter::Coreclr, 31337, None, Platform::Linux).unwrap();
    session::start(&client, &plan(StartKind::Attach, p.arguments), T).unwrap();
    assert_eq!(handle.commands()[1], "attach");
    assert_eq!(handle.last("attach").unwrap()["processId"], 31337);
    assert_eq!(process_event(&rec, 1), Some(31337));
    // An attached session breaks like a launched one.
    handle.trigger();
    let s = rec.stopped(1);
    assert_eq!(s.reason, "breakpoint");
    // Detaching (the default after attach) ends the session without the program exiting.
    client
        .request_wait("disconnect", json!({"terminateDebuggee": false}), T)
        .unwrap();
    rec.wait_nth(1, "terminated", |e| {
        matches!(e, ClientEvent::Event(Event::Terminated))
    });
    assert!(
        !rec.events()
            .iter()
            .any(|e| matches!(e, ClientEvent::Event(Event::Exited(_)))),
        "detaching does not end the program"
    );
}

#[test]
fn restart_starts_over_where_the_adapter_has_it_and_is_refused_where_not() {
    let mut p = program();
    p.extra_capabilities = json!({"supportsRestartRequest": true});
    let (conn, handle) = fake::connect(p);
    let rec = Recorder::default();
    let client = DapClient::start(conn, rec.sink());
    let started = session::start(
        &client,
        &plan(StartKind::Launch, json!({"program": "/s/App.dll"})),
        T,
    )
    .unwrap();
    assert!(started.capabilities.supports_restart_request);
    handle.trigger();
    assert_eq!(rec.stopped(1).reason, "breakpoint");
    client.request_wait("restart", json!({}), T).unwrap();
    // A new process event, and the program runs from the start again: the same breakpoint stops it.
    process_event(&rec, 2);
    handle.trigger();
    let s = rec.stopped(2);
    assert_eq!(s.reason, "breakpoint");
    let st = client
        .request_wait("stackTrace", json!({"threadId": s.thread_id.unwrap()}), T)
        .unwrap();
    assert_eq!(st["stackFrames"][0]["line"], 6);

    // Without the capability the request is refused.
    let (conn, handle) = fake::connect(program());
    let rec = Recorder::default();
    let client = DapClient::start(conn, rec.sink());
    let started = session::start(
        &client,
        &plan(StartKind::Launch, json!({"program": "/s/App.dll"})),
        T,
    )
    .unwrap();
    assert!(!started.capabilities.supports_restart_request);
    match client.request_wait("restart", json!({}), T) {
        Err(DapError::Failed { message, .. }) => assert!(message.contains("restart"), "{message}"),
        other => panic!("{other:?}"),
    }
    drop(handle);
}
