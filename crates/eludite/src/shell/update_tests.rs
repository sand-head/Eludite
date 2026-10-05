//! Headless tests of the self-updater in the shell (brief 0055) against `eludite-update`'s loopback release server:
//! a development build says it cannot update and never asks the network; a packaged build in `download` mode
//! checks on its timer, stages the newer build, shows "Restart to update" and, on `eludite.update.apply` from an
//! agent's thread or the status bar's click with the confirmation, hands the applier its plan; the first-start
//! question writes `updates.mode` and starts checking; Help > Check for Updates on the newest build says so.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use eludite_commands::update::{APPLY, CHECK, STATUS};
use eludite_ui::{slot_selector, slots};
use eludite_update::apply::{Plan, Relaunch};
use eludite_update::download::Source;
use eludite_update::http::UreqTransport;
use eludite_update::test_support::{Server, install, list, publish};
use eludite_update::updater::Setup;
use eludite_update::{Build, Platform};
use gpui::TestAppContext;
use serde_json::{Value, json};

use super::tests::{USER_SETTINGS, Ws, setup, setup_services};
use super::update::{TICK, UPDATE_SLOT, UpdateSetup};

const REPO: &str = "sand-head/Eludite";
/// The test clock is advanced past these by hand (GPUI's test timers wait for `advance_clock`).
const ASK_DELAY: Duration = Duration::from_millis(50);
const FIRST_CHECK: Duration = Duration::from_millis(400);
const OLD: &str = "unstable-20261005.9";
const NEW: &str = "unstable-20261006.3";

/// A release server with `NEW` published, an install folder at `OLD`, and the shell over them.
struct Packaged {
    w: Ws,
    server: Server,
    install_dir: PathBuf,
    plans: Arc<Mutex<Vec<Plan>>>,
    _dir: tempfile::TempDir,
}

fn packaged(cx: &mut TestAppContext, installed: &str, ask: bool) -> Packaged {
    let dir = tempfile::tempdir().unwrap();
    let server = Server::serve(dir.path());
    let base = server.base();
    let release = publish(
        dir.path(),
        &base,
        NEW,
        "linux",
        "x86_64",
        &[
            ("eludite", b"new binary", 0o755),
            ("README", b"new readme", 0o644),
        ],
    );
    list(dir.path(), REPO, &[release]);
    let install_dir = install(dir.path(), installed, "linux", "x86_64");
    let plans: Arc<Mutex<Vec<Plan>>> = Arc::default();
    let record = plans.clone();
    let setup = UpdateSetup {
        setup: Setup {
            install_dir: install_dir.clone(),
            platform: Platform {
                os: "linux".into(),
                arch: "x86_64".into(),
            },
            build: Build::read(&install_dir),
            state_file: Some(dir.path().join("state/updates.json")),
            source: Source {
                api: base,
                repository: REPO.into(),
            },
            transport: Arc::new(UreqTransport::new(Duration::from_secs(20))),
        },
        relaunch: Relaunch {
            args: vec!["--folder".into(), "/work/app".into()],
            cwd: None,
        },
        restarter: Arc::new(move |plan: &Plan| {
            record.lock().unwrap().push(plan.clone());
            Ok(plan.install_dir.join(".eludite-update/apply/plan.json"))
        }),
        quit_on_apply: false,
        ask,
        ask_delay: ASK_DELAY,
        first_check_delay: FIRST_CHECK,
        updated_from: None,
    };
    let w = setup_services(cx, |_| {}, None, None, move |s| s.update_setup = setup);
    Packaged {
        w,
        server,
        install_dir,
        plans,
        _dir: dir,
    }
}

impl Ws {
    fn write_user_settings(&self, value: Value) {
        let p = self.path(USER_SETTINGS);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        let tmp = p.with_extension("tmp");
        std::fs::write(&tmp, serde_json::to_string_pretty(&value).unwrap()).unwrap();
        std::fs::rename(&tmp, &p).unwrap();
    }

    fn update_status(&self) -> Value {
        self.commands.invoke(STATUS, json!({})).unwrap()
    }

    fn update_slot(&self) -> String {
        self.shell.read_with(&self.vcx, |s, _| {
            s.status().get(UPDATE_SLOT).unwrap_or_default().to_owned()
        })
    }

    fn state_slot(&self) -> String {
        self.shell.read_with(&self.vcx, |s, _| {
            s.status().get(slots::STATE).unwrap_or_default().to_owned()
        })
    }

    fn update_output(&self) -> Vec<String> {
        self.shell.read_with(&self.vcx, |s, cx| {
            s.output
                .read(cx)
                .pane(eludite_commands::build::OutputSource::Updates)
                .tail(100)
        })
    }

    /// Invoke `command` from another thread while the UI keeps running (the command may need the UI thread).
    fn invoke_while_running(&mut self, command: &str, args: Value) -> Result<Value, String> {
        let c = self.commands.clone();
        let name = command.to_owned();
        let handle = std::thread::spawn(move || c.invoke(&name, args).map_err(|e| e.to_string()));
        let deadline = Instant::now() + super::tests::T;
        while !handle.is_finished() {
            self.vcx.run_until_parked();
            assert!(Instant::now() < deadline, "{command} did not finish");
            std::thread::sleep(Duration::from_millis(5));
        }
        handle.join().unwrap()
    }

    fn run_from_ui(&mut self, command: &str, args: Value) {
        self.shell.update_in(&mut self.vcx, |s, window, cx| {
            s.run(command, args, window, cx);
        });
        self.vcx.run_until_parked();
    }
}

fn kind(status: &Value) -> String {
    status["state"]["kind"]
        .as_str()
        .unwrap_or_default()
        .to_owned()
}

#[gpui::test]
fn a_development_build_cannot_update_and_never_asks_the_network(cx: &mut TestAppContext) {
    let mut w = setup(cx);
    let s = w.update_status();
    assert_eq!(s["enabled"], false, "{s}");
    assert!(
        s["reason"].as_str().unwrap().contains("development build"),
        "{s}"
    );
    assert_eq!(kind(&s), "idle");
    // Help > Check for Updates: the command answers with the same, and nothing runs.
    w.run_from_ui(CHECK, json!({}));
    let s = w.update_status();
    assert_eq!(s["enabled"], false);
    assert_eq!(s["busy"], false);
    assert_eq!(w.update_slot(), "");
    // An agent's apply is refused with a reason, not a crash.
    let err = w.invoke_while_running(APPLY, json!({})).unwrap_err();
    assert!(err.contains("no update is staged"), "{err}");
}

#[gpui::test]
fn download_mode_stages_the_newer_build_and_apply_hands_the_applier_its_plan(
    cx: &mut TestAppContext,
) {
    let mut p = packaged(cx, OLD, false);
    p.w.write_user_settings(json!({"updates.mode": "download"}));
    p.w.wait("the mode", |w| w.update_status()["mode"] == "download");
    // The timer's first check (FIRST_CHECK after the start) finds the newer build and stages it.
    p.w.vcx.executor().advance_clock(FIRST_CHECK);
    p.w.wait("the staged build", |w| kind(&w.update_status()) == "ready");
    let s = p.w.update_status();
    assert_eq!(s["state"]["update"]["tag"], NEW, "{s}");
    assert_eq!(s["enabled"], true);
    assert!(s["last_check"].is_string());
    assert_eq!(p.w.update_slot(), format!("Restart to update ({NEW})"));
    let lines = p.w.update_output();
    assert!(
        lines
            .iter()
            .any(|l| l.contains("A newer build is available")),
        "{lines:?}"
    );
    assert!(
        lines
            .iter()
            .any(|l| l.contains("verified against SHA256SUMS")),
        "{lines:?}"
    );
    assert!(lines.iter().any(|l| l.contains("staged in")), "{lines:?}");
    let layout = p
        .install_dir
        .join(".eludite-update")
        .join(NEW)
        .join("layout");
    assert_eq!(
        std::fs::read(layout.join("eludite")).unwrap(),
        b"new binary"
    );
    let before = p.server.paths().len();

    // An agent restarts from its own thread: the UI thread writes the plan and answers.
    let out = p.w.invoke_while_running(APPLY, json!({})).unwrap();
    assert_eq!(out["applying"], true);
    assert_eq!(out["tag"], NEW);
    let plans = p.plans.lock().unwrap().clone();
    assert_eq!(plans.len(), 1);
    let plan = &plans[0];
    assert_eq!(plan.install_dir, p.install_dir);
    assert_eq!(plan.layout_dir, layout);
    assert_eq!(plan.to.tag(), NEW);
    assert_eq!(plan.from.as_ref().unwrap().tag(), OLD);
    assert_eq!(plan.executable, "eludite");
    assert_eq!(
        plan.relaunch.as_ref().unwrap().args,
        vec!["--folder".to_owned(), "/work/app".to_owned()]
    );

    // The person clicks the status bar: the confirmation, then the same plan.
    p.w.click(&slot_selector(UPDATE_SLOT));
    p.w.vcx.simulate_prompt_answer("Restart Now");
    p.w.wait("the second plan", |_| p.plans.lock().unwrap().len() == 2);
    let audit = p.w.audit();
    assert_eq!(audit.iter().filter(|c| *c == APPLY).count(), 2, "{audit:?}");
    assert_eq!(p.server.paths().len(), before, "no further network call");
}

#[gpui::test]
fn the_first_start_asks_once_and_the_answer_starts_checking(cx: &mut TestAppContext) {
    let mut p = packaged(cx, OLD, true);
    p.w.vcx.executor().advance_clock(ASK_DELAY);
    p.w.wait("the question", |w| {
        w.shell.read_with(&w.vcx, |s, _| s.update.asked)
    });
    p.w.vcx.simulate_prompt_answer("Only tell me");
    p.w.wait("the answer in the user file", |w| {
        std::fs::read_to_string(w.path(USER_SETTINGS))
            .ok()
            .and_then(|t| serde_json::from_str::<Value>(&t).ok())
            .is_some_and(|v| v["updates.mode"] == "notify")
    });
    // `notify` checks and tells, but downloads nothing.
    p.w.wait("the check", |w| kind(&w.update_status()) == "available");
    assert_eq!(p.w.update_slot(), format!("Update available: {NEW}"));
    let paths = p.server.paths();
    assert!(paths.iter().all(|p| p.starts_with("/repos/")), "{paths:?}");
    assert_eq!(p.w.update_status()["mode"], "notify");
    assert!(!p.install_dir.join(".eludite-update").join(NEW).exists());
}

#[gpui::test]
fn check_for_updates_on_the_newest_build_says_it_is_up_to_date(cx: &mut TestAppContext) {
    let mut p = packaged(cx, NEW, false);
    p.w.write_user_settings(json!({"updates.mode": "off"}));
    p.w.wait("the mode", |w| w.update_status()["mode"] == "off");
    p.w.run_from_ui(CHECK, json!({}));
    p.w.wait("the answer", |w| kind(&w.update_status()) == "up_to_date");
    p.w.wait("the status text", |w| {
        w.state_slot() == format!("Eludite is up to date ({NEW})")
    });
    assert_eq!(p.w.update_slot(), "");
    assert_eq!(p.server.paths().len(), 1);
    // `off` never checks again on its own: the timer stays quiet.
    p.w.vcx.executor().advance_clock(FIRST_CHECK + TICK * 2);
    p.w.vcx.run_until_parked();
    std::thread::sleep(Duration::from_millis(100));
    p.w.vcx.run_until_parked();
    assert_eq!(p.server.paths().len(), 1);
    assert!(Path::new(&p.install_dir).join("build.json").is_file());
}
