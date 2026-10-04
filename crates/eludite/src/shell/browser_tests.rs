//! Headless tests of the shell's browser (brief 0023) against a fake [`Engine`]: the commands are registered and
//! agent-visible with their schemas, they run on the `browser` worker and never on (or against) the UI thread, the
//! Output window's Browser source gets the lifecycle lines, the settings reach the launch, and the browser closes
//! with the workspace. The real Chrome is `crates/browser`'s tests.
//!
//! Brief 0024, with the scripted fake agent calling the tools through the MCP endpoint: the policy gate prompts for a
//! navigation off the allowed origins (class dangerous, with the reason) and lets a local one through, Always Allow
//! adds the origin to the policy file, `browser.evaluate: deny` and `browser.network_bodies: deny` refuse, and a
//! screenshot in a tool result becomes a thumbnail in the Agents window that opens the full image.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

use eludite_browser::{
    CdpEvent, Engine, EngineConfig, EngineError, LaunchInfo, LogSink, TargetInfo,
};
use eludite_commands::browser as cmds;
use eludite_commands::build::OutputSource;
use eludite_commands::settings::SET;
use eludite_commands::workspace;
use serde_json::{Value, json};

use super::agents::window::Decision;
use super::tests::{Ws, setup, setup_full};

/// What the fake engines of one test saw.
#[derive(Default)]
struct Seen {
    /// How long `is_running` blocks, to stand for a slow browser.
    delay: Mutex<Duration>,
    /// The configuration of each launch.
    launches: Mutex<Vec<EngineConfig>>,
    shutdowns: AtomicUsize,
    /// The threads the engine was called on.
    threads: Mutex<Vec<String>>,
    /// Event channels handed out (kept open, as a live tab's are).
    events: Mutex<Vec<mpsc::Sender<CdpEvent>>>,
}

struct FakeEngine {
    seen: Arc<Seen>,
    config: EngineConfig,
    log: LogSink,
    running: bool,
}

impl FakeEngine {
    fn record(&self) {
        let name = std::thread::current().name().unwrap_or("?").to_owned();
        self.seen.threads.lock().unwrap().push(name);
    }
}

impl Engine for FakeEngine {
    fn name(&self) -> &'static str {
        "fake-chrome"
    }

    fn configure(&mut self, config: EngineConfig) {
        self.config = config;
    }

    fn is_running(&self) -> bool {
        self.record();
        std::thread::sleep(*self.seen.delay.lock().unwrap());
        self.running
    }

    fn launch(&mut self) -> Result<Option<LaunchInfo>, EngineError> {
        if self.running {
            return Ok(None);
        }
        self.running = true;
        self.seen.launches.lock().unwrap().push(self.config.clone());
        (self.log)("Launched fake-chrome (Fake/1.0); DevTools at ws://fake");
        Ok(self.info())
    }

    fn info(&self) -> Option<LaunchInfo> {
        self.running.then(|| LaunchInfo {
            executable: "fake-chrome".into(),
            version: "Fake/1.0".into(),
            endpoint: "ws://fake".into(),
        })
    }

    fn targets(&self) -> Result<Vec<TargetInfo>, EngineError> {
        if !self.running {
            return Err(EngineError::NotRunning);
        }
        Ok(vec![TargetInfo {
            target_id: "T1".into(),
            url: "about:blank".into(),
            title: String::new(),
        }])
    }

    fn open_tab(&mut self, _url: &str) -> Result<String, EngineError> {
        Ok("T2".into())
    }

    fn close_tab(&mut self, _target_id: &str) -> Result<(), EngineError> {
        Ok(())
    }

    fn activate_tab(&mut self, _target_id: &str) -> Result<(), EngineError> {
        Ok(())
    }

    fn attach(&mut self, target_id: &str) -> Result<String, EngineError> {
        Ok(format!("S-{target_id}"))
    }

    fn send(
        &self,
        _session: &str,
        method: &str,
        _params: Value,
        _timeout: Duration,
    ) -> Result<Value, EngineError> {
        self.record();
        Ok(match method {
            "Page.getLayoutMetrics" => {
                let layout =
                    json!({"pageX": 0, "pageY": 0, "clientWidth": 1280, "clientHeight": 800});
                let visual = json!({"offsetX": 0, "offsetY": 0, "pageX": 0, "pageY": 0, "clientWidth": 1280,
                    "clientHeight": 800, "scale": 1});
                let size = json!({"x": 0, "y": 0, "width": 1280, "height": 800});
                json!({"layoutViewport": layout, "visualViewport": visual, "contentSize": size,
                    "cssLayoutViewport": layout, "cssVisualViewport": visual, "cssContentSize": size})
            }
            "Runtime.evaluate" => json!({"result": {"type": "number", "value": 1}}),
            // No loader: a same-document navigation, done when it answers.
            "Page.navigate" => json!({"frameId": "F"}),
            _ => json!({}),
        })
    }

    fn screenshot(&self, _: &str, _: Value, _: Duration) -> Result<String, EngineError> {
        Ok(super::agents::transcript::tests::png(640, 400))
    }

    fn subscribe(&self, _session: &str) -> Result<mpsc::Receiver<CdpEvent>, EngineError> {
        let (tx, rx) = mpsc::channel();
        self.seen.events.lock().unwrap().push(tx);
        Ok(rx)
    }

    fn shutdown(&mut self) {
        if self.running {
            self.running = false;
            self.seen.shutdowns.fetch_add(1, Ordering::SeqCst);
            (self.log)("Closed the browser.");
        }
    }
}

/// The shell with fake engines.
fn setup_fake(cx: &mut gpui::TestAppContext) -> (Ws, Arc<Seen>) {
    let w = setup(cx);
    fake_engines(w)
}

fn fake_engines(w: Ws) -> (Ws, Arc<Seen>) {
    let seen = Arc::new(Seen::default());
    let engines = seen.clone();
    w.shell.read_with(&w.vcx, |s, _| {
        s.browser().set_engine_factory(Arc::new(move |config, log| {
            Box::new(FakeEngine {
                seen: engines.clone(),
                config,
                log,
                running: false,
            })
        }))
    });
    (w, seen)
}

fn browser_output(w: &Ws) -> String {
    w.shell.read_with(&w.vcx, |s, cx| {
        let pane = s.output.read(cx).pane(OutputSource::Browser);
        (0..pane.len())
            .filter_map(|i| pane.line(i))
            .collect::<Vec<_>>()
            .join("\n")
    })
}

#[gpui::test]
fn browser_commands_are_registered_agent_visible_with_their_schemas(cx: &mut gpui::TestAppContext) {
    let (w, seen) = setup_fake(cx);
    for id in cmds::ALL {
        let spec = w
            .commands
            .lookup(id)
            .unwrap_or_else(|| panic!("{id} is registered"));
        assert!(spec.agent_visible, "{id}");
        let expected = cmds::spec(id);
        assert_eq!(spec.input_schema, expected.input_schema, "{id}");
        assert_eq!(spec.output_schema, expected.output_schema, "{id}");
        assert_eq!(spec.permission, expected.permission, "{id}");
    }
    assert!(
        w.commands
            .agent_visible()
            .iter()
            .any(|s| s.id.as_str() == cmds::SCREENSHOT)
    );
    // Registering is all a cold start pays for the browser: the twenty commands' schemas.
    let fresh = eludite_commands::CommandRegistry::new();
    let t = Instant::now();
    let _bus = super::browser::register(&fresh);
    let took = t.elapsed();
    eprintln!("registering the browser commands at startup: {took:?}");
    assert!(took < Duration::from_millis(50), "{took:?}");
    // A cold start does nothing for the browser: no worker, no engine, nothing launched.
    assert!(!w.shell.read_with(&w.vcx, |s, _| s.browser().started()));
    assert!(seen.threads.lock().unwrap().is_empty());
}

#[gpui::test]
fn tabs_runs_on_the_browser_worker_and_never_delays_a_frame(cx: &mut gpui::TestAppContext) {
    let (mut w, seen) = setup_fake(cx);
    *seen.delay.lock().unwrap() = Duration::from_millis(200);
    // The UI thread never calls into the browser: it fails at once rather than wait.
    let t = Instant::now();
    let err = w
        .commands
        .invoke(cmds::TABS, json!({}))
        .unwrap_err()
        .to_string();
    assert!(err.contains("UI thread"), "{err}");
    assert!(t.elapsed() < Duration::from_millis(50));
    assert!(seen.threads.lock().unwrap().is_empty());

    // An agent's call (another thread) waits on the worker while the UI keeps drawing.
    let commands = w.commands.clone();
    let agent = std::thread::spawn(move || commands.invoke(cmds::TABS, json!({})));
    let started = Instant::now();
    let mut worst = Duration::ZERO;
    let mut frames = 0;
    while !agent.is_finished() {
        assert!(
            started.elapsed() < super::tests::T,
            "the agent's call timed out"
        );
        let t = Instant::now();
        w.shell.update(&mut w.vcx, |_, cx| cx.notify());
        w.vcx.run_until_parked();
        worst = worst.max(t.elapsed());
        frames += 1;
        std::thread::sleep(Duration::from_millis(2));
    }
    let out = agent.join().unwrap().unwrap();
    assert!(
        started.elapsed() >= Duration::from_millis(200),
        "the fake engine blocked"
    );
    assert_eq!(out["running"], false);
    assert_eq!(out["engine"]["name"], "fake-chrome");
    assert!(frames > 5, "frames went on during the call ({frames})");
    assert!(
        worst < Duration::from_millis(100),
        "a frame waited {worst:?} on the browser"
    );
    let threads = seen.threads.lock().unwrap().clone();
    assert!(
        !threads.is_empty() && threads.iter().all(|t| t == super::browser::THREAD),
        "{threads:?}"
    );
    eprintln!("tabs with a 200 ms engine: {frames} UI updates meanwhile, the slowest {worst:?}");
}

#[gpui::test]
fn the_output_window_shows_the_browsers_lifecycle(cx: &mut gpui::TestAppContext) {
    let (mut w, _seen) = setup_fake(cx);
    let opened = w.agent_invoke(cmds::TAB_OPEN, json!({})).unwrap();
    assert_eq!(opened["id"], "t1");
    assert_eq!(opened["launched"], true);
    w.wait("the launch and the tab in Output", |w| {
        let out = browser_output(w);
        out.contains("Launched fake-chrome (Fake/1.0)") && out.contains("Opened tab t1")
    });
    let tabs = w.agent_invoke(cmds::TABS, json!({})).unwrap();
    assert_eq!(tabs["running"], true);
    assert_eq!(tabs["engine"]["version"], "Fake/1.0");
    w.agent_invoke(cmds::TAB_CLOSE, json!({"tab": "t1"}))
        .unwrap();
    w.wait("the closed tab in Output", |w| {
        browser_output(w).contains("Closed tab t1")
    });
    // Every call is audited as every command is.
    let audit = w.audit();
    for id in [cmds::TAB_OPEN, cmds::TABS, cmds::TAB_CLOSE] {
        assert!(audit.iter().any(|c| c == id), "{id} in {audit:?}");
    }
}

#[gpui::test]
fn settings_reach_the_launch_and_the_browser_closes_with_the_workspace(
    cx: &mut gpui::TestAppContext,
) {
    let (mut w, seen) = setup_fake(cx);
    w.open_solution();
    for (key, value) in [
        ("browser.chromePath", json!("/opt/chrome-test/chrome")),
        ("browser.headless", json!(true)),
        ("browser.viewport", json!("800x600")),
    ] {
        w.agent_invoke(SET, json!({"key": key, "value": value}))
            .unwrap();
    }
    w.wait("the browser settings applied", |w| {
        w.shell
            .read_with(&w.vcx, |s, _| s.browser().settings().viewport == (800, 600))
    });
    w.agent_invoke(cmds::TAB_OPEN, json!({})).unwrap();
    let launch = seen.launches.lock().unwrap()[0].clone();
    assert_eq!(
        launch.executable.as_deref(),
        Some(std::path::Path::new("/opt/chrome-test/chrome"))
    );
    assert!(launch.headless);
    assert_eq!(launch.viewport, (800, 600));
    assert_eq!(
        launch.profile_dir,
        w.dir
            .path()
            .join(".eludite")
            .join("browser")
            .join("profile")
    );

    // Closing the workspace closes the browser, without waiting on the UI thread.
    let commands = w.commands.clone();
    let agent = std::thread::spawn(move || commands.invoke(workspace::WORKSPACE_CLOSE, json!({})));
    w.wait("the workspace closed", |_| agent.is_finished());
    w.wait("the browser closed", |w| {
        seen.shutdowns.load(Ordering::SeqCst) == 1
            && browser_output(w).contains("Closed the browser.")
    });
    // The next launch (no workspace open) uses a profile outside any workspace.
    let next = w.shell.read_with(&w.vcx, |s, _| s.browser().config());
    assert!(
        !next.profile_dir.starts_with(w.dir.path()),
        "{}",
        next.profile_dir.display()
    );

    // Shell exit: the shutdown answers once the browser is closed.
    w.agent_invoke(cmds::TAB_OPEN, json!({})).unwrap();
    let done = w.shell.read_with(&w.vcx, |s, _| s.browser().shutdown());
    done.recv_timeout(super::tests::T).unwrap();
    assert_eq!(seen.shutdowns.load(Ordering::SeqCst), 2);
}

/// The shell with fake engines, the solution open, its policy file, and one scripted fake agent per entry, each
/// running its steps (`{"tool": ..., "arguments": ...}`) through the MCP endpoint when prompted.
fn setup_scripted(
    cx: &mut gpui::TestAppContext,
    policy: Value,
    agents: &[(&str, Value)],
) -> (Ws, Arc<Seen>, tempfile::TempDir) {
    let mut setup = super::agents::tests::fake_agents(
        agents
            .iter()
            .map(|(name, steps)| {
                (
                    (*name).to_owned(),
                    vec![
                        "--scenario".into(),
                        "script".into(),
                        "--script".into(),
                        steps.to_string(),
                    ],
                )
            })
            .collect(),
    );
    // The transcript (and the images opened from it) go to a folder of their own.
    let out = tempfile::tempdir().unwrap();
    setup.transcript_out = Some(out.path().join("transcript.json"));
    let mut w = setup_full(cx, |_| {}, Some(setup));
    w.open_solution();
    let file = eludite_commands::policy::AgentPolicy::path_for(w.dir.path());
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(&file, policy.to_string()).unwrap();
    let (w, seen) = fake_engines(w);
    (w, seen, out)
}

impl Ws {
    /// Start agent `name` and send it a prompt.
    fn run_agent(&mut self, name: &str) {
        let name = name.to_owned();
        self.shell
            .update(&mut self.vcx, |s, cx| {
                s.agents_start(Some(&name), true, cx)?;
                s.agents_prompt("go", cx)
            })
            .unwrap();
    }

    fn turn_over(&self) -> bool {
        self.shell.read_with(&self.vcx, |s, _| {
            s.agents().last_stop.is_some()
                && s.agents().state != super::agents::window::StateKind::Running
        })
    }

    fn prompt(&self) -> Option<super::agents::window::Prompt> {
        self.shell
            .read_with(&self.vcx, |s, cx| s.agents().window.read(cx).prompt.clone())
    }

    fn step(&self, n: usize) -> Value {
        let rows = self.shell.read_with(&self.vcx, |s, cx| {
            s.agents().window.read(cx).transcript.to_json()
        });
        rows.as_array()
            .unwrap()
            .iter()
            .find(|r| r["tool_call"]["id"] == format!("toolu_fake_step_{n}"))
            .map(|r| r["tool_call"].clone())
            .unwrap_or(Value::Null)
    }

    fn reset_turn(&mut self) {
        self.shell
            .update(&mut self.vcx, |s, _| s.agents.last_stop = None);
    }
}

#[gpui::test]
fn the_gate_prompts_off_the_allowed_origins_and_always_allow_adds_the_origin(
    cx: &mut gpui::TestAppContext,
) {
    let steps = json!([
        {"tool": "eludite-browser-tab_open"},
        {"tool": "eludite-browser-navigate", "arguments": {"url": "http://127.0.0.1:4321/form.html"}},
        {"tool": "eludite-browser-navigate", "arguments": {"url": "https://example.com/"}}
    ]);
    let (mut w, _seen, _out) = setup_scripted(
        cx,
        json!({"version": 1, "execute": "allow"}),
        &[("Navigator", steps)],
    );
    w.run_agent("Navigator");
    // The local navigation runs (execute, allowed by the policy); the one to example.com waits for the user.
    w.wait("the permission prompt", |w| w.prompt().is_some());
    let p = w.prompt().unwrap();
    assert_eq!(p.class, "dangerous");
    assert_eq!(
        p.reason.as_deref(),
        Some("navigate off the allowed origins: https://example.com")
    );
    assert!(p.can_persist);
    assert!(p.tool.contains("eludite-browser-navigate"), "{}", p.tool);
    let local = w.step(2);
    assert_eq!(local["status"], "completed", "{local}");
    assert!(
        local["note"].as_str().unwrap().contains("(execute)"),
        "{local}"
    );
    // The window says why.
    w.commands
        .invoke(
            "eludite.view.show",
            json!({"id": eludite_docking::ids::AGENTS}),
        )
        .unwrap();
    w.vcx.run_until_parked();
    assert!(w.vcx.debug_bounds("agents-permission-dialog").is_some());

    let answered = w
        .shell
        .update(&mut w.vcx, |s, cx| {
            s.agents_answer(p.request, Decision::AlwaysAllow, cx)
        })
        .unwrap();
    assert!(answered.persisted);
    assert_eq!(answered.class, eludite_commands::PermissionClass::Dangerous);
    w.wait("the turn's end", |w| w.turn_over());
    let far = w.step(3);
    assert_eq!(far["status"], "completed", "{far}");
    assert!(
        far["note"]
            .as_str()
            .unwrap()
            .contains("always for this solution (the origin https://example.com)"),
        "{far}"
    );
    // The policy file now lists the defaults and the origin, sorted.
    let file = eludite_commands::policy::AgentPolicy::path_for(w.dir.path());
    w.wait("the policy file written", |_| {
        std::fs::read_to_string(&file).is_ok_and(|t| t.contains("https://example.com"))
    });
    let saved: Value = serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
    assert_eq!(
        saved["browser"]["origins"],
        json!([
            "$launch_urls",
            "$workspace",
            "127.0.0.1",
            "[::1]",
            "https://example.com",
            "localhost"
        ])
    );
    assert_eq!(saved["execute"], "allow");
    // Every call is audited with its effective class and the reason.
    let navigations: Vec<_> = w
        .commands
        .audit_log()
        .entries()
        .into_iter()
        .filter(|e| e.command == cmds::NAVIGATE)
        .map(|e| (e.permission, e.escalation.clone(), e.is_ok()))
        .collect();
    assert_eq!(
        navigations,
        [
            (Some(eludite_commands::PermissionClass::Execute), None, true),
            (
                Some(eludite_commands::PermissionClass::Dangerous),
                Some("navigate off the allowed origins: https://example.com".into()),
                true
            ),
        ]
    );

    // The next turn: example.com is an allowed origin now; nothing prompts.
    w.reset_turn();
    w.run_agent("Navigator");
    w.wait("the second turn's end", |w| w.turn_over());
    assert!(w.prompt().is_none());
    let classes: Vec<_> = w
        .commands
        .audit_log()
        .entries()
        .into_iter()
        .filter(|e| e.command == cmds::NAVIGATE)
        .map(|e| e.permission)
        .collect();
    assert_eq!(
        classes[2..],
        [Some(eludite_commands::PermissionClass::Execute); 2]
    );
}

#[gpui::test]
fn a_denied_off_origin_navigation_fails_and_the_policy_refuses_evaluate_and_bodies(
    cx: &mut gpui::TestAppContext,
) {
    let steps = json!([
        {"tool": "eludite-browser-tab_open"},
        {"tool": "eludite-browser-evaluate", "arguments": {"expression": "1 + 1"}},
        {"tool": "eludite-browser-network_body", "arguments": {"request_id": "1.1"}},
        {"tool": "eludite-browser-navigate", "arguments": {"url": "https://example.com/"}}
    ]);
    let (mut w, _seen, _out) = setup_scripted(
        cx,
        json!({"version": 1, "execute": "allow", "browser": {"evaluate": "deny", "network_bodies": "deny"}}),
        &[("Refused", steps)],
    );
    w.run_agent("Refused");
    w.wait("the permission prompt", |w| w.prompt().is_some());
    // evaluate and network_body were refused without asking, naming the policy.
    for (n, policy) in [(2, "browser.evaluate"), (3, "browser.network_bodies")] {
        let row = w.step(n);
        assert_eq!(row["status"], "failed", "{row}");
        let text = row["result"].as_str().unwrap();
        assert!(
            text.contains("permission denied") && text.contains(policy),
            "{text}"
        );
    }
    let p = w.prompt().unwrap();
    w.shell
        .update(&mut w.vcx, |s, cx| {
            s.agents_answer(p.request, Decision::Deny, cx)
        })
        .unwrap();
    w.wait("the turn's end", |w| w.turn_over());
    let row = w.step(4);
    assert_eq!(row["status"], "denied", "{row}");
    assert!(
        row["result"]
            .as_str()
            .unwrap()
            .contains("is class dangerous")
    );
    let refused: Vec<_> = w
        .commands
        .audit_log()
        .entries()
        .into_iter()
        .filter(|e| e.command == cmds::EVALUATE || e.command == cmds::NETWORK_BODY)
        .map(|e| {
            (
                e.command.clone(),
                e.escalation.clone().unwrap_or_default(),
                e.is_ok(),
            )
        })
        .collect();
    assert_eq!(
        refused,
        [
            (
                cmds::EVALUATE.to_owned(),
                "the solution's policy sets browser.evaluate to deny".to_owned(),
                false
            ),
            (
                cmds::NETWORK_BODY.to_owned(),
                "the solution's policy sets browser.network_bodies to deny".to_owned(),
                false
            ),
        ]
    );
}

#[gpui::test]
fn a_screenshot_in_a_tool_result_is_a_thumbnail_that_opens_the_image(
    cx: &mut gpui::TestAppContext,
) {
    let steps = json!([
        {"tool": "eludite-browser-tab_open"},
        {"tool": "eludite-browser-screenshot"}
    ]);
    let (mut w, _seen, _out) = setup_scripted(
        cx,
        json!({"version": 1, "execute": "allow"}),
        &[("Photographer", steps)],
    );
    // The system viewer is a script that records what it is asked to open (a shell script: Unix only).
    let opened = w.dir.path().join("opened.txt");
    let opener = w.dir.path().join("opener.sh");
    if cfg!(unix) {
        std::fs::write(
            &opener,
            format!(
                "#!/bin/sh\nprintf '%s\\n' \"$1\" >> '{}'\n",
                opened.display()
            ),
        )
        .unwrap();
        std::process::Command::new("chmod")
            .args(["+x", &opener.to_string_lossy()])
            .status()
            .unwrap();
    }
    w.shell.read_with(&w.vcx, |s, _| {
        s.browser()
            .set_opener(Some(opener.to_string_lossy().into_owned()))
    });
    w.commands
        .invoke(
            "eludite.view.show",
            json!({"id": eludite_docking::ids::AGENTS}),
        )
        .unwrap();
    w.run_agent("Photographer");
    w.wait("the turn's end", |w| w.turn_over());
    w.wait("the thumbnail", |w| !w.step(2)["images"].is_null());
    let shot = w.step(2);
    assert_eq!(shot["status"], "completed", "{shot}");
    // Once, though the image came both in Eludite's result and in the agent's forwarded content.
    assert_eq!(
        shot["images"],
        json!([{"mime": "image/png", "width": 640, "height": 400, "thumb_width": 160, "thumb_height": 100}])
    );
    assert!(w.step(1)["images"].is_null());
    // It is drawn in the transcript row, and a click opens the full image with the system viewer.
    let row = w.shell.read_with(&w.vcx, |s, cx| {
        s.agents()
            .window
            .read(cx)
            .transcript
            .rows
            .iter()
            .position(|r| matches!(r, super::agents::transcript::Row::Tool(t) if t.call.tool_call_id == "toolu_fake_step_2"))
            .unwrap()
    });
    let sel = super::agents::window::thumb(row, 0);
    w.wait("the thumbnail drawn", |w| {
        let sel: &'static str = Box::leak(sel.clone().into_boxed_str());
        w.vcx.debug_bounds(sel).is_some()
    });
    let b = w.bounds(&sel);
    assert!(b.size.width <= gpui::px(162.), "{b:?}");
    if !cfg!(unix) {
        return;
    }
    w.click(&sel);
    w.wait("the image opened", |_| {
        std::fs::read_to_string(&opened).is_ok_and(|t| !t.is_empty())
    });
    let url = std::fs::read_to_string(&opened).unwrap();
    let url = url.trim();
    assert!(
        url.starts_with("file://") && url.ends_with("toolu_fake_step_2-0.png"),
        "{url}"
    );
    let path = url.trim_start_matches("file://");
    let saved = image::open(path).unwrap();
    assert_eq!((saved.width(), saved.height()), (640, 400));
    assert!(
        w.commands
            .audit_log()
            .entries()
            .iter()
            .any(|e| e.command == cmds::OPEN_EXTERNAL && e.is_ok()),
        "the open is a command, audited"
    );
}

// ---- A fake embedded engine for the launch tests (brief 0037) ----

/// What [`PageEngine`]s saw: the pages navigated to (by engine tab), reloads, closed tabs.
#[derive(Default)]
pub(crate) struct PageSeen {
    /// `Page.navigate`: (engine tab, url), in order.
    pub navigated: Mutex<Vec<(String, String)>>,
    /// `Page.reload`: the engine tabs reloaded, in order.
    pub reloads: Mutex<Vec<String>>,
    /// The open engine tabs and their pages.
    pub pages: Mutex<Vec<(String, String)>>,
    next: AtomicUsize,
    events: Mutex<Vec<(String, mpsc::Sender<CdpEvent>)>>,
}

impl PageSeen {
    /// The page engine tab `target` shows.
    pub fn url_of(&self, target: &str) -> Option<String> {
        self.pages
            .lock()
            .unwrap()
            .iter()
            .find(|(t, _)| t == target)
            .map(|(_, u)| u.clone())
    }

    /// A main-frame navigation that loads, as Chromium reports one (`Page.reload` waits for it).
    fn loaded(&self, target: &str, url: &str) {
        let loader = format!("L{}", self.next.fetch_add(1, Ordering::SeqCst));
        for (session, tx) in self.events.lock().unwrap().iter() {
            if session != target {
                continue;
            }
            for (method, params) in [
                (
                    "Page.frameNavigated",
                    json!({"frame": {"id": "F", "loaderId": loader, "url": url}}),
                ),
                (
                    "Page.lifecycleEvent",
                    json!({"frameId": "F", "loaderId": loader, "name": "load"}),
                ),
            ] {
                let _ = tx.send(CdpEvent {
                    method: method.into(),
                    params,
                    session_id: Some(session.clone()),
                });
            }
        }
    }
}

/// A fake engine standing for the embedded one: tabs with pages, `navigate`, `reload` (with its load events) and
/// the window's history.
pub(crate) struct PageEngine {
    seen: Arc<PageSeen>,
    running: bool,
}

impl Engine for PageEngine {
    fn name(&self) -> &'static str {
        "embedded-chromium"
    }
    fn configure(&mut self, _config: EngineConfig) {}
    fn is_running(&self) -> bool {
        self.running
    }
    fn launch(&mut self) -> Result<Option<LaunchInfo>, EngineError> {
        if self.running {
            return Ok(None);
        }
        self.running = true;
        Ok(Some(LaunchInfo::default()))
    }
    fn info(&self) -> Option<LaunchInfo> {
        self.running.then(LaunchInfo::default)
    }
    fn targets(&self) -> Result<Vec<TargetInfo>, EngineError> {
        if !self.running {
            return Err(EngineError::NotRunning);
        }
        Ok(self
            .seen
            .pages
            .lock()
            .unwrap()
            .iter()
            .map(|(t, u)| TargetInfo {
                target_id: t.clone(),
                url: u.clone(),
                // A web project's page has the corpus page's title (brief 0038 names a browser session after it).
                title: if u.starts_with("http") {
                    "Minimal API".into()
                } else {
                    String::new()
                },
            })
            .collect())
    }
    /// A port the fake js-debug never connects to (brief 0038): the shell only passes it on.
    fn debug_endpoint(&self) -> Option<eludite_browser::DebugEndpoint> {
        self.running.then(|| eludite_browser::DebugEndpoint {
            address: "127.0.0.1".into(),
            port: 9,
        })
    }
    fn cdp_target_id(&self, target_id: &str) -> Option<String> {
        Some(format!("TARGET-{target_id}"))
    }
    fn open_tab(&mut self, url: &str) -> Result<String, EngineError> {
        let id = format!("P{}", self.seen.next.fetch_add(1, Ordering::SeqCst) + 1);
        self.seen
            .pages
            .lock()
            .unwrap()
            .push((id.clone(), url.to_owned()));
        Ok(id)
    }
    fn close_tab(&mut self, target: &str) -> Result<(), EngineError> {
        self.seen.pages.lock().unwrap().retain(|(t, _)| t != target);
        Ok(())
    }
    fn activate_tab(&mut self, _target: &str) -> Result<(), EngineError> {
        Ok(())
    }
    fn attach(&mut self, target: &str) -> Result<String, EngineError> {
        Ok(target.to_owned())
    }
    fn send(
        &self,
        session: &str,
        method: &str,
        params: Value,
        _timeout: Duration,
    ) -> Result<Value, EngineError> {
        let url = self.seen.url_of(session).unwrap_or_default();
        Ok(match method {
            "Page.navigate" => {
                let to = params["url"].as_str().unwrap_or_default().to_owned();
                self.seen
                    .navigated
                    .lock()
                    .unwrap()
                    .push((session.to_owned(), to.clone()));
                if let Some(p) = self
                    .seen
                    .pages
                    .lock()
                    .unwrap()
                    .iter_mut()
                    .find(|(t, _)| t == session)
                {
                    p.1 = to;
                }
                // No loader: done when it answers.
                json!({"frameId": "F"})
            }
            "Page.reload" => {
                self.seen.reloads.lock().unwrap().push(session.to_owned());
                self.seen.loaded(session, &url);
                json!({})
            }
            "Runtime.evaluate" => {
                json!({"result": {"type": "object", "value": {"u": url, "t": "Minimal API"}}})
            }
            "Page.getNavigationHistory" => json!({"currentIndex": 0, "entries": [
                {"id": 0, "url": url, "userTypedURL": url, "title": "", "transitionType": "typed"}]}),
            _ => json!({}),
        })
    }
    fn subscribe(&self, session: &str) -> Result<mpsc::Receiver<CdpEvent>, EngineError> {
        let (tx, rx) = mpsc::channel();
        self.seen
            .events
            .lock()
            .unwrap()
            .push((session.to_owned(), tx));
        Ok(rx)
    }
    fn history(&self, _target: &str) -> Option<eludite_browser::TabHistory> {
        Some(eludite_browser::TabHistory::default())
    }
    fn shutdown(&mut self) {
        self.running = false;
        self.seen.pages.lock().unwrap().clear();
    }
}

/// The shell's browser draws [`PageEngine`]s (as the embedded engine's tabs) from now on.
pub(crate) fn install_page_engine(w: &Ws) -> Arc<PageSeen> {
    let seen = Arc::new(PageSeen::default());
    let engines = seen.clone();
    w.shell.read_with(&w.vcx, |s, _| {
        s.browser()
            .set_engine_factory(Arc::new(move |_config, _log: LogSink| {
                Box::new(PageEngine {
                    seen: engines.clone(),
                    running: false,
                })
            }))
    });
    seen
}
