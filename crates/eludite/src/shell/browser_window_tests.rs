//! Headless tests of the Web Browser window (brief 0032) against a fake engine that stands for the embedded one: it
//! answers the commands as an [`Engine`] and tells the window what `eludite-chromium` would ([`WindowEvent`]s through
//! [`super::browser::BrowserBus::window_sink`]), with a [`PageDriver`] that records what the window sends the tabs.
//!
//! The window opens from View > Other Windows > Web Browser with a tab; the address bar navigates through the bus;
//! Back and Forward follow the history; the tab strip follows `tabs`; Ctrl+T and Ctrl+W; the "Agent is driving" strip
//! while a fake agent's call is in flight, its Stop and the person's click interrupting the agent's `wait`, and the
//! agent's next action refused until it reads `tabs`; a dialog and a permission request from the engine as shell
//! dialogs whose answers reach the engine; the context menu; DevTools as a tab; downloads' Output line; closing the
//! window (the engine lingers) and the workspace (the engine closes); and the engine-missing message. Brief 0039: the
//! engine's sandbox refusal opens the opt-in dialog once per workspace; declining leaves the engine off with the
//! message; accepting stores `browser.allowNoSandbox`, starts the engine with the opt-in, audits the start and shows
//! the strip; the setting off again starts the next engine without it; nothing is searched for at startup. Brief 0047:
//! Accept writes the person's state for the workspace (mode 0600), never the workspace's `.eludite/settings.json`; a
//! `true` in that file is ignored, with the warning in the window and the Output window, and the dialog still comes;
//! a stored answer from an earlier session starts the engine with no dialog; the audit entry and `tabs` name what
//! allowed the start (`dialog`, `options`, `variable`); `ELUDITE_CHROME_NO_SANDBOX=1` still works.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

use eludite_browser::embedded::{Frame, FrameSource};
use eludite_browser::{
    CdpEvent, ChromiumSearch, Engine, EngineConfig, EngineError, LaunchInfo, LogSink, TabHistory,
    TargetInfo,
};
use eludite_commands::browser as cmds;
use eludite_commands::build::OutputSource;
use eludite_commands::{Caller, next_call_id, with_caller};
use eludite_docking::ids::WEB_BROWSER;
use gpui::{Modifiers, MouseButton, MouseDownEvent, MouseUpEvent};
use serde_json::{Value, json};

use super::browser::{PageDriver, WindowEvent, WindowSink};
use super::tests::{Ws, setup};

/// Frames that change on every look.
#[derive(Default)]
struct Moving(AtomicU64);

impl FrameSource for Moving {
    fn sequence(&self) -> u64 {
        self.0.fetch_add(1, Ordering::SeqCst) + 1
    }

    fn read(&self, _consume: bool, f: &mut dyn FnMut(&Frame<'_>)) -> bool {
        let seq = self.0.load(Ordering::SeqCst);
        let pixels = vec![(seq % 250) as u8; 64 * 32 * 4];
        f(&Frame {
            sequence: seq,
            width: 64,
            height: 32,
            stride: 256,
            pixels: &pixels,
            dirty: &[],
            paint_ns: 0,
            copy_ns: 0,
        });
        true
    }

    fn set_listener(&self, _f: Option<Box<dyn Fn() + Send + Sync>>) {}
}

/// What the fake engine and its driver saw, and its pages' history.
#[derive(Default)]
struct Seen {
    /// Each tab's history (urls) and current index.
    history: Mutex<HashMap<String, (Vec<String>, usize)>>,
    open: Mutex<Vec<String>>,
    next: AtomicUsize,
    sent: Mutex<Vec<(String, Value)>>,
    /// What the window sent the tabs: `tab/input` events and notifications.
    inputs: Mutex<Vec<(String, Value)>>,
    notified: Mutex<Vec<(String, Value)>>,
    closed: Mutex<Vec<String>>,
    launches: AtomicUsize,
    shutdowns: AtomicUsize,
    sink: Mutex<Option<WindowSink>>,
    events: Mutex<Vec<mpsc::Sender<CdpEvent>>>,
    /// Brief 0039: the fake refuses to start without the opt-in, as `eludite-chromium` as root or with neither user
    /// namespaces nor the setuid helper; each launch attempt's opt-in; running without the sandbox.
    refuse_sandbox: std::sync::atomic::AtomicBool,
    attempts: Mutex<Vec<bool>>,
    unsandboxed: std::sync::atomic::AtomicBool,
}

/// The engine's refusal where neither user namespaces nor the helper work (brief 0039's wording).
const REFUSAL: &str = "Chromium's sandbox cannot start on this machine: unprivileged user namespaces are not \
    available, and the setuid helper /opt/eludite/cef/chrome-sandbox is missing. Either allow user namespaces (`sudo \
    sysctl -w kernel.unprivileged_userns_clone=1`, or on Ubuntu 23.10 and later an AppArmor profile that permits them \
    for eludite-chromium), or install the helper with `sudo chown root:root /opt/eludite/cef/chrome-sandbox && sudo \
    chmod 4755 /opt/eludite/cef/chrome-sandbox`, or let this workspace run the browser without the sandbox (the Web \
    Browser window offers it; the setting browser.allowNoSandbox; ELUDITE_CHROME_NO_SANDBOX=1 for tests), which \
    starts the engine with --allow-no-sandbox.";

impl Seen {
    fn tell(&self, e: WindowEvent) {
        if let Some(s) = self.sink.lock().unwrap().as_ref() {
            s(e);
        }
    }

    fn state(&self, tab: &str) {
        let (url, back, forward) = {
            let h = self.history.lock().unwrap();
            let (urls, ix) = &h[tab];
            (urls[*ix].clone(), *ix > 0, ix + 1 < urls.len())
        };
        self.tell(WindowEvent::Notification {
            method: "tab/state".into(),
            params: json!({"tab": tab, "url": url, "title": format!("Page {url}"), "loading": false,
                "canGoBack": back, "canGoForward": forward}),
        });
    }
}

struct Driver(Arc<Seen>);

impl PageDriver for Driver {
    fn frames(&self, _target: &str) -> Option<Arc<dyn FrameSource>> {
        Some(Arc::new(Moving::default()))
    }
    fn input(&self, target: &str, event: Value) {
        self.0
            .inputs
            .lock()
            .unwrap()
            .push((target.to_owned(), event));
    }
    fn resize(&self, _target: &str, _w: u32, _h: u32, _s: f32) {}
    fn notify(&self, method: &str, params: Value) {
        self.0
            .notified
            .lock()
            .unwrap()
            .push((method.to_owned(), params));
    }
    fn close(&self, target: &str) {
        self.0.closed.lock().unwrap().push(target.to_owned());
    }
    fn sandboxed(&self) -> bool {
        !self.0.unsandboxed.load(Ordering::SeqCst)
    }
}

struct FakeEmbedded {
    seen: Arc<Seen>,
    running: bool,
    /// The configuration's `allow_no_sandbox` (the shell's `--allow-no-sandbox`).
    allow_no_sandbox: bool,
}

impl Engine for FakeEmbedded {
    fn name(&self) -> &'static str {
        "embedded-chromium"
    }
    fn configure(&mut self, config: EngineConfig) {
        self.allow_no_sandbox = config.allow_no_sandbox;
    }
    fn is_running(&self) -> bool {
        self.running
    }
    fn launch(&mut self) -> Result<Option<LaunchInfo>, EngineError> {
        if self.running {
            return Ok(None);
        }
        self.seen
            .attempts
            .lock()
            .unwrap()
            .push(self.allow_no_sandbox);
        let refuse = self.seen.refuse_sandbox.load(Ordering::SeqCst);
        if refuse && !self.allow_no_sandbox {
            self.seen
                .tell(WindowEvent::SandboxRefused(REFUSAL.to_owned()));
            return Err(EngineError::Launch(REFUSAL.to_owned()));
        }
        self.seen.unsandboxed.store(refuse, Ordering::SeqCst);
        self.running = true;
        self.seen.launches.fetch_add(1, Ordering::SeqCst);
        self.seen
            .tell(WindowEvent::Started(Arc::new(Driver(self.seen.clone()))));
        Ok(self.info())
    }
    fn info(&self) -> Option<LaunchInfo> {
        // As `engine/ready` says it (brief 0039): `none` when it runs without the sandbox.
        self.running.then(|| LaunchInfo {
            sandbox: self
                .seen
                .unsandboxed
                .load(Ordering::SeqCst)
                .then(|| "none".to_owned()),
            ..LaunchInfo::default()
        })
    }
    fn targets(&self) -> Result<Vec<TargetInfo>, EngineError> {
        if !self.running {
            return Err(EngineError::NotRunning);
        }
        let h = self.seen.history.lock().unwrap();
        Ok(self
            .seen
            .open
            .lock()
            .unwrap()
            .iter()
            .map(|t| TargetInfo {
                target_id: t.clone(),
                url: h.get(t).map(|(u, i)| u[*i].clone()).unwrap_or_default(),
                title: String::new(),
            })
            .collect())
    }
    fn open_tab(&mut self, url: &str) -> Result<String, EngineError> {
        let id = (self.seen.next.fetch_add(1, Ordering::SeqCst) + 1).to_string();
        self.seen.open.lock().unwrap().push(id.clone());
        self.seen
            .history
            .lock()
            .unwrap()
            .insert(id.clone(), (vec![url.to_owned()], 0));
        Ok(id)
    }
    fn close_tab(&mut self, target: &str) -> Result<(), EngineError> {
        self.seen.open.lock().unwrap().retain(|t| t != target);
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
        self.seen
            .sent
            .lock()
            .unwrap()
            .push((method.to_owned(), params.clone()));
        let tab = session;
        Ok(match method {
            "Page.navigate" => {
                {
                    let mut h = self.seen.history.lock().unwrap();
                    let (urls, ix) = h.get_mut(tab).unwrap();
                    urls.truncate(*ix + 1);
                    urls.push(params["url"].as_str().unwrap_or_default().to_owned());
                    *ix = urls.len() - 1;
                }
                self.seen.state(tab);
                // No loader: a same-document navigation, done when it answers.
                json!({"frameId": "F"})
            }
            "Page.getNavigationHistory" => {
                let h = self.seen.history.lock().unwrap();
                let (urls, ix) = &h[tab];
                let entries: Vec<Value> = urls
                    .iter()
                    .enumerate()
                    .map(|(i, u)| json!({"id": i, "url": u, "userTypedURL": u, "title": "", "transitionType": "typed"}))
                    .collect();
                json!({"currentIndex": ix, "entries": entries})
            }
            "Page.navigateToHistoryEntry" => {
                self.seen.history.lock().unwrap().get_mut(tab).unwrap().1 =
                    params["entryId"].as_u64().unwrap_or(0) as usize;
                self.seen.state(tab);
                json!({})
            }
            "Runtime.evaluate" => {
                let e = params["expression"].as_str().unwrap_or_default();
                if e.contains("location.href") {
                    let h = self.seen.history.lock().unwrap();
                    let (urls, ix) = &h[tab];
                    json!({"result": {"type": "object", "value": {"u": urls[*ix], "t": ""}}})
                } else {
                    json!({"result": {"type": "boolean", "value": false}})
                }
            }
            _ => json!({}),
        })
    }
    fn subscribe(&self, _s: &str) -> Result<mpsc::Receiver<CdpEvent>, EngineError> {
        let (tx, rx) = mpsc::channel();
        self.seen.events.lock().unwrap().push(tx);
        Ok(rx)
    }
    fn history(&self, target: &str) -> Option<TabHistory> {
        let h = self.seen.history.lock().unwrap();
        let (urls, ix) = h.get(target)?;
        Some(TabHistory {
            can_go_back: *ix > 0,
            can_go_forward: ix + 1 < urls.len(),
            favicon: String::new(),
        })
    }
    fn devtools(
        &mut self,
        target: &str,
        _inspect: Option<(f64, f64)>,
    ) -> Result<bool, EngineError> {
        self.seen.tell(WindowEvent::DevtoolsOpened {
            page: target.to_owned(),
            devtools: format!("dt{target}"),
        });
        Ok(true)
    }
    fn shutdown(&mut self) {
        if self.running {
            self.running = false;
            self.seen.open.lock().unwrap().clear();
            self.seen.shutdowns.fetch_add(1, Ordering::SeqCst);
            self.seen.tell(WindowEvent::Stopped);
        }
    }
}

fn setup_window(cx: &mut gpui::TestAppContext) -> (Ws, Arc<Seen>) {
    let w = setup(cx);
    let seen = Arc::new(Seen::default());
    let engines = seen.clone();
    w.shell.read_with(&w.vcx, |s, _| {
        *seen.sink.lock().unwrap() = Some(s.browser().window_sink());
        s.browser()
            .set_engine_factory(Arc::new(move |config: EngineConfig, _log: LogSink| {
                Box::new(FakeEmbedded {
                    seen: engines.clone(),
                    running: false,
                    allow_no_sandbox: config.allow_no_sandbox,
                })
            }))
    });
    (w, seen)
}

impl Ws {
    fn browser_window<R>(&self, f: impl FnOnce(&super::browser_window::BrowserWindow) -> R) -> R {
        self.shell
            .read_with(&self.vcx, |s, cx| f(s.browser_window().read(cx)))
    }

    fn with_window(
        &mut self,
        f: impl FnOnce(
            &mut super::browser_window::BrowserWindow,
            &mut gpui::Context<super::browser_window::BrowserWindow>,
        ),
    ) {
        let bw = self
            .shell
            .read_with(&self.vcx, |s, _| s.browser_window().clone());
        bw.update(&mut self.vcx, f);
        self.vcx.run_until_parked();
    }

    /// View > Other Windows > Web Browser, as the menu item runs it.
    fn open_web_browser(&mut self) {
        self.shell.update_in(&mut self.vcx, |s, window, cx| {
            s.run(
                eludite_commands::view::SHOW,
                json!({"id": WEB_BROWSER}),
                window,
                cx,
            )
        });
        self.vcx.run_until_parked();
    }

    fn strip(&self) -> Vec<(String, String, bool)> {
        self.browser_window(|b| b.strip())
    }

    fn agent_thread(
        &self,
        id: &'static str,
        args: Value,
    ) -> std::thread::JoinHandle<Result<Value, String>> {
        let commands = self.commands.clone();
        std::thread::spawn(move || {
            let caller = Caller::Agent {
                agent: "Fake Agent".into(),
                call: next_call_id(),
                tool_call: None,
            };
            with_caller(caller, || commands.invoke(id, args)).map_err(|e| e.to_string())
        })
    }

    fn agent_call(&self, id: &'static str, args: Value) -> Result<Value, String> {
        self.agent_thread(id, args).join().unwrap()
    }
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
fn the_window_opens_from_the_view_menu_with_a_tab_and_navigates_through_the_bus(
    cx: &mut gpui::TestAppContext,
) {
    let (mut w, seen) = setup_window(cx);
    // Nothing at startup: no engine, the window closed.
    assert_eq!(seen.launches.load(Ordering::SeqCst), 0);
    assert!(!w.browser_window(|b| b.is_open()));
    assert_eq!(
        w.shell.read_with(&w.vcx, |s, cx| s
            .menu()
            .read(cx)
            .is_item_enabled("View", "Other Windows > Web Browser")),
        Some(true)
    );
    let t0 = Instant::now();
    w.open_web_browser();
    assert_eq!(
        w.controller.active_document().as_deref(),
        Some(WEB_BROWSER),
        "a document tab"
    );
    w.wait("the window's first tab", |w| {
        let s = w.strip();
        s.len() == 1 && s[0].2
    });
    w.wait("the first page pixel", |w| {
        w.shell
            .read_with(&w.vcx, |s, cx| s.browser_window().read(cx).first_pixel(cx))
            .is_some()
    });
    eprintln!(
        "window open to its first tab and page pixel (fake engine): {:?}",
        t0.elapsed()
    );
    assert_eq!(seen.launches.load(Ordering::SeqCst), 1);
    let ran = w.browser_window(|b| b.ran().to_vec());
    assert_eq!(
        ran[0],
        (cmds::TAB_OPEN.to_owned(), json!({"url": "about:blank"}))
    );
    // The window's commands are the person's, audited as any.
    assert!(w.audit().iter().any(|c| c == cmds::TAB_OPEN));

    // The address bar navigates through eludite.browser.navigate.
    w.with_window(|b, cx| {
        b.type_address("localhost:5000/orders", cx);
        b.go(cx);
    });
    w.wait("the navigation", |w| {
        w.browser_window(|b| b.address() == "http://localhost:5000/orders" && b.can_go().0)
    });
    // The window's state updates on the bus's answer; the fake engine records the CDP message on its own thread.
    w.wait("Page.navigate at the engine", |_| {
        seen.sent
            .lock()
            .unwrap()
            .iter()
            .any(|(m, p)| m == "Page.navigate" && p["url"] == "http://localhost:5000/orders")
    });
    w.wait("the navigate's audit entry", |w| {
        w.audit().iter().any(|c| c == cmds::NAVIGATE)
    });
    assert_eq!(w.browser_window(|b| b.can_go()), (true, false));
    assert_eq!(
        w.browser_window(|b| b.history().to_vec()),
        ["http://localhost:5000/orders"]
    );

    // Back and Forward enable and disable with the history (Alt+Left, then the toolbar's Forward).
    w.vcx.simulate_keystrokes("alt-left");
    // (The tab's history: about:blank, the home page about:blank, the orders page.)
    w.wait("back", |w| w.browser_window(|b| b.can_go()) == (true, true));
    assert_eq!(w.browser_window(|b| b.address().to_owned()), "about:blank");
    w.click("web-browser-forward");
    w.wait("forward", |w| {
        w.browser_window(|b| b.can_go()) == (true, false)
    });
    // F5 reloads, only inside the window (elsewhere F5 is Start Debugging).
    w.vcx.simulate_keystrokes("f5");
    w.wait("the reload", |_| {
        seen.sent
            .lock()
            .unwrap()
            .iter()
            .any(|(m, _)| m == "Page.reload")
    });
    // Ctrl+L selects the whole address: typing replaces it, Backspace edits, Enter goes there.
    w.vcx
        .simulate_keystrokes("ctrl-l x y z . c o m backspace m enter");
    w.wait("the typed address", |_| {
        seen.sent
            .lock()
            .unwrap()
            .iter()
            .any(|(m, p)| m == "Page.navigate" && p["url"] == "http://xyz.com")
    });
    assert_eq!(
        w.browser_window(|b| b.address().to_owned()),
        "http://xyz.com"
    );
}

#[gpui::test]
fn the_tab_strip_follows_the_tabs_and_ctrl_t_and_ctrl_w(cx: &mut gpui::TestAppContext) {
    let (mut w, _seen) = setup_window(cx);
    w.open_web_browser();
    w.wait("one tab", |w| w.strip().len() == 1);
    // An agent's tab_open shows in the strip, selected (the commands' active tab).
    w.agent_call(cmds::TAB_OPEN, json!({"url": "http://localhost:5000/"}))
        .unwrap();
    w.wait("the agent's tab", |w| {
        let s = w.strip();
        s.len() == 2 && s[1].2
    });
    assert_eq!(w.strip()[1].0, "Page http://localhost:5000/");
    // Ctrl+T opens a tab, Ctrl+W closes it; clicking a tab selects it.
    w.vcx.simulate_keystrokes("ctrl-t");
    w.wait("Ctrl+T", |w| w.strip().len() == 3 && w.strip()[2].2);
    w.vcx.simulate_keystrokes("ctrl-w");
    w.wait("Ctrl+W", |w| w.strip().len() == 2);
    w.click("web-browser-tab-0");
    w.wait("the first tab selected", |w| w.strip()[0].2);
    let tabs = w.agent_call(cmds::TABS, json!({})).unwrap();
    assert_eq!(
        tabs["active"], "t1",
        "the window selects through tab_select"
    );
    // The + button.
    w.click("web-browser-new-tab");
    w.wait("+", |w| w.strip().len() == 3);
}

#[gpui::test]
fn the_agent_strip_shows_while_an_agent_drives_and_stop_or_a_click_interrupts_its_wait(
    cx: &mut gpui::TestAppContext,
) {
    let (mut w, seen) = setup_window(cx);
    w.open_web_browser();
    w.wait("one tab", |w| w.strip().len() == 1);

    // A wait in flight: the strip names the agent; Stop ends the wait with interrupted_by.
    let call = w.agent_thread(
        cmds::WAIT,
        json!({"for": "selector", "css": "#never", "wait_ms": 20000}),
    );
    w.wait("the strip", |w| {
        w.browser_window(|b| b.driving().to_vec()) == ["Fake Agent"]
    });
    assert!(w.vcx.debug_bounds("web-browser-agent-strip").is_some());
    let t0 = Instant::now();
    w.click("web-browser-agent-stop");
    let out = call.join().unwrap().unwrap();
    // Stopped, not timed out: far under the 20 s wait.
    assert!(t0.elapsed() < Duration::from_secs(10), "{:?}", t0.elapsed());
    assert_eq!(out["interrupted_by"], "user", "{out}");
    w.wait("the strip gone", |w| {
        w.browser_window(|b| b.driving().is_empty())
    });
    assert!(w.vcx.debug_bounds("web-browser-agent-strip").is_none());

    // The interrupted agent's next action is refused until it reads tabs (its page_generation).
    let refused = w
        .agent_call(cmds::NAVIGATE, json!({"url": "http://localhost:5000/"}))
        .unwrap_err();
    assert!(refused.contains("eludite.browser.tabs"), "{refused}");
    w.agent_call(cmds::TABS, json!({})).unwrap();
    w.agent_call(cmds::NAVIGATE, json!({"url": "http://localhost:5000/"}))
        .unwrap();

    // The person's click in the page ends the next wait the same way.
    let call = w.agent_thread(
        cmds::WAIT,
        json!({"for": "selector", "css": "#never", "wait_ms": 20000}),
    );
    w.wait("the strip again", |w| {
        !w.browser_window(|b| b.driving().is_empty())
    });
    let page = w
        .vcx
        .debug_bounds("browser-surface")
        .map(|b| b.center())
        .unwrap_or_else(|| gpui::point(gpui::px(600.), gpui::px(400.)));
    w.vcx.simulate_event(MouseDownEvent {
        position: page,
        modifiers: Modifiers::none(),
        button: MouseButton::Left,
        click_count: 1,
        first_mouse: false,
    });
    w.vcx.simulate_event(MouseUpEvent {
        position: page,
        modifiers: Modifiers::none(),
        button: MouseButton::Left,
        click_count: 1,
    });
    w.vcx.run_until_parked();
    let out = call.join().unwrap().unwrap();
    assert_eq!(out["interrupted_by"], "user", "{out}");
    assert!(
        seen.inputs
            .lock()
            .unwrap()
            .iter()
            .any(|(_, e)| e["type"] == "mouseDown"),
        "the click reached the page"
    );
}

#[gpui::test]
fn dialogs_and_permission_requests_are_shell_dialogs_answered_through_the_engine(
    cx: &mut gpui::TestAppContext,
) {
    let (mut w, seen) = setup_window(cx);
    w.open_web_browser();
    w.wait("one tab", |w| w.strip().len() == 1);
    let target = w.strip()[0].1.clone();

    seen.tell(WindowEvent::Notification {
        method: "tab/dialog".into(),
        params: json!({"tab": target, "id": 5, "kind": "confirm", "message": "Delete the order?"}),
    });
    w.wait("the dialog", |w| {
        w.browser_window(|b| b.prompts().len()) == 1
    });
    assert!(w.vcx.debug_bounds("web-browser-prompt").is_some());
    w.click("web-browser-prompt-ok");
    let answers = seen.notified.lock().unwrap().clone();
    assert_eq!(
        answers.last().unwrap(),
        &(
            "tab/dialogAnswer".to_owned(),
            json!({"tab": target, "id": 5, "accept": true})
        )
    );
    assert!(w.browser_window(|b| b.prompts().is_empty()));

    // A prompt typed into.
    seen.tell(WindowEvent::Notification {
        method: "tab/dialog".into(),
        params: json!({"tab": target, "id": 6, "kind": "prompt", "message": "Name?", "defaultText": ""}),
    });
    w.wait("the prompt", |w| {
        w.browser_window(|b| b.prompts().len()) == 1
    });
    w.vcx.simulate_keystrokes("A d a enter");
    let last = seen.notified.lock().unwrap().last().cloned().unwrap();
    assert_eq!(last.1["text"], "Ada", "{last:?}");
    // Answered, the dialog gives the keys back to the page.
    w.vcx.run_until_parked();
    seen.inputs.lock().unwrap().clear();
    w.vcx.simulate_keystrokes("q");
    assert!(
        seen.inputs
            .lock()
            .unwrap()
            .iter()
            .any(|(t, e)| *t == target && e["type"] == "rawKeyDown"),
        "the page has the keys"
    );

    // A permission request, blocked.
    seen.tell(WindowEvent::Notification {
        method: "tab/permission".into(),
        params: json!({"tab": target, "id": 7, "origin": "http://localhost:5000", "permissions": ["geolocation"]}),
    });
    w.wait("the permission request", |w| {
        w.browser_window(|b| b.prompts().first().map(|p| p.message.clone()))
            == Some("http://localhost:5000 wants to use: geolocation".into())
    });
    w.click("web-browser-prompt-cancel");
    assert_eq!(
        seen.notified.lock().unwrap().last().unwrap(),
        &(
            "tab/permissionAnswer".to_owned(),
            json!({"tab": target, "id": 7, "allow": false})
        )
    );

    // An agent's answer (the engine's dialogClosed) closes the shell's dialog.
    seen.tell(WindowEvent::Notification {
        method: "tab/dialog".into(),
        params: json!({"tab": target, "id": 8, "kind": "alert", "message": "Saved"}),
    });
    w.wait("the alert", |w| {
        w.browser_window(|b| b.prompts().len()) == 1
    });
    seen.tell(WindowEvent::Notification {
        method: "tab/dialogClosed".into(),
        params: json!({"tab": target, "id": 8, "accepted": true}),
    });
    w.wait("the alert gone", |w| {
        w.browser_window(|b| b.prompts().is_empty())
    });
}

#[gpui::test]
fn the_context_menu_devtools_downloads_and_the_cursor(cx: &mut gpui::TestAppContext) {
    let (mut w, seen) = setup_window(cx);
    w.open_web_browser();
    w.wait("one tab", |w| w.strip().len() == 1);
    let target = w.strip()[0].1.clone();

    // The engine's context menu request shows ours; Copy Link and Copy run.
    seen.tell(WindowEvent::Notification {
        method: "tab/contextMenu".into(),
        params: json!({"tab": target, "x": 40, "y": 30, "pageUrl": "http://localhost:5000/",
            "linkUrl": "http://localhost:5000/orders/7", "editable": false, "edit": {}}),
    });
    w.wait("the menu", |w| {
        w.browser_window(|b| b.context_menu().is_some())
    });
    w.click("web-browser-menu-Copy Link");
    assert_eq!(
        w.vcx.read_from_clipboard().and_then(|c| c.text()),
        Some("http://localhost:5000/orders/7".into())
    );
    seen.tell(WindowEvent::Notification {
        method: "tab/contextMenu".into(),
        params: json!({"tab": target, "x": 40, "y": 30, "pageUrl": "http://localhost:5000/", "editable": true, "edit": {}}),
    });
    w.wait("the menu again", |w| {
        w.browser_window(|b| b.context_menu().is_some())
    });
    w.click("web-browser-menu-Copy");
    assert_eq!(
        seen.notified.lock().unwrap().last().unwrap(),
        &(
            "tab/action".to_owned(),
            json!({"tab": target, "action": "copy"})
        )
    );

    // F12: DevTools as a tab beside its page, shown; its close button closes DevTools only.
    w.vcx.simulate_keystrokes("f12");
    w.wait("DevTools", |w| w.strip().len() == 2);
    let strip = w.strip();
    assert!(strip[1].0.starts_with("DevTools - "), "{strip:?}");
    assert!(strip[1].2, "shown");
    assert!(w.audit().iter().any(|c| c == cmds::DEVTOOLS));
    seen.tell(WindowEvent::Notification {
        method: "tab/state".into(),
        params: json!({"tab": format!("dt{target}"), "url": "devtools://devtools/bundled/inspector.html"}),
    });
    w.wait("DevTools' address", |w| {
        w.browser_window(|b| b.address().starts_with("devtools:"))
    });
    w.click("web-browser-tab-close-1");
    w.wait("DevTools closed", |w| w.strip().len() == 1);
    assert_eq!(
        seen.closed.lock().unwrap().as_slice(),
        [format!("dt{target}")]
    );
    // The address bar shows the page's address again.
    assert_eq!(w.browser_window(|b| b.address().to_owned()), "about:blank");

    // Downloads: the window's status line, and the Output window's line at the end.
    seen.tell(WindowEvent::Notification {
        method: "tab/download".into(),
        params: json!({"tab": target, "id": 1, "url": "http://localhost:5000/report.csv", "file": "report.csv",
            "state": "complete", "path": "/w/.eludite/browser/downloads/report.csv", "receivedBytes": 42}),
    });
    w.wait("the Output line", |w| {
        browser_output(w).contains(
            "Downloaded http://localhost:5000/report.csv to /w/.eludite/browser/downloads/report.csv (42 bytes)",
        )
    });
    assert!(w.browser_window(|b| b.status_line().contains("report.csv")));

    // The page's cursor.
    seen.tell(WindowEvent::Notification {
        method: "tab/cursor".into(),
        params: json!({"tab": target, "cursor": "pointer"}),
    });
    w.vcx.run_until_parked();
}

#[gpui::test]
fn closing_the_window_lingers_and_closing_the_workspace_closes_the_engine(
    cx: &mut gpui::TestAppContext,
) {
    let (mut w, seen) = setup_window(cx);
    w.shell.read_with(&w.vcx, |s, _| {
        s.browser().set_linger(Duration::from_millis(300))
    });
    w.open_web_browser();
    w.wait("one tab", |w| w.strip().len() == 1);
    // Closed and reopened within the linger: the same engine, the same tab, nothing launched.
    w.controller.close_document(WEB_BROWSER);
    w.vcx.run_until_parked();
    assert!(!w.browser_window(|b| b.is_open()));
    w.open_web_browser();
    std::thread::sleep(Duration::from_millis(500));
    w.vcx.run_until_parked();
    assert_eq!(seen.shutdowns.load(Ordering::SeqCst), 0);
    assert_eq!(w.strip().len(), 1);
    // Closed for longer than the linger: the engine closes, its tabs leave the window.
    w.controller.close_document(WEB_BROWSER);
    w.wait("the engine closed after the linger", |_| {
        seen.shutdowns.load(Ordering::SeqCst) == 1
    });
    w.wait("no tab", |w| w.strip().is_empty());

    // The workspace's close closes it at once.
    w.open_solution();
    w.open_web_browser();
    w.wait("a tab again", |w| w.strip().len() == 1);
    assert_eq!(seen.launches.load(Ordering::SeqCst), 2);
    w.commands
        .invoke(eludite_commands::workspace::SOLUTION_CLOSE, json!({}))
        .unwrap();
    w.wait("the engine closed with the workspace", |_| {
        seen.shutdowns.load(Ordering::SeqCst) == 2
    });
}

#[gpui::test]
fn without_the_embedded_engine_the_window_says_what_to_run(cx: &mut gpui::TestAppContext) {
    let mut w = setup(cx);
    w.shell.read_with(&w.vcx, |s, _| {
        s.browser().set_chromium_search(ChromiumSearch::default())
    });
    w.open_web_browser();
    let message = w
        .browser_window(|b| b.message().map(str::to_owned))
        .unwrap();
    if cfg!(target_os = "linux") {
        assert!(message.contains("tools/cef/fetch.sh"), "{message}");
    }
    assert!(w.vcx.debug_bounds("web-browser-message").is_some());
    // Nothing was launched for it: the window does not open a tab without the engine.
    assert!(w.browser_window(|b| b.ran().is_empty()));
    assert!(!w.shell.read_with(&w.vcx, |s, _| s.browser().started()));

    // browser.engine: external says so, and how to come back.
    w.controller.close_document(WEB_BROWSER);
    w.vcx.run_until_parked();
    w.agent_invoke(
        eludite_commands::settings::SET,
        json!({"key": "browser.engine", "value": "external"}),
    )
    .unwrap();
    w.wait("the setting applied", |w| {
        w.shell.read_with(&w.vcx, |s, _| {
            s.browser().settings().engine == eludite_browser::EngineChoice::External
        })
    });
    w.open_web_browser();
    let message = w
        .browser_window(|b| b.message().map(str::to_owned))
        .unwrap();
    assert!(message.contains("browser.engine"), "{message}");
}

/// The workspace's `browser.allowNoSandbox`: its value and where it came from.
fn allow_setting(w: &Ws) -> (Value, String) {
    let out = settings_get(w);
    let row = &out["settings"][0];
    (
        row["value"].clone(),
        row["source"].as_str().unwrap_or_default().to_owned(),
    )
}

/// `eludite.settings.get` of `browser.allowNoSandbox`.
fn settings_get(w: &Ws) -> Value {
    w.commands
        .invoke(
            eludite_commands::settings::GET,
            json!({"key": "browser.allowNoSandbox"}),
        )
        .unwrap()
}

/// The person's state file for the open workspace (brief 0047), as `eludite.settings.get` names it.
fn state_file(w: &mut Ws) -> std::path::PathBuf {
    w.wait("the workspace's state", |w| {
        settings_get(w)["user_workspace_file"]["path"].is_string()
    });
    let out = settings_get(w);
    std::path::PathBuf::from(out["user_workspace_file"]["path"].as_str().unwrap())
}

/// The audit entries of engine starts without the sandbox.
fn engine_starts(w: &Ws) -> Vec<Value> {
    w.commands
        .audit_log()
        .entries()
        .into_iter()
        .filter(|e| e.command == super::browser_window::ENGINE_START)
        .filter_map(|e| e.arguments)
        .collect()
}

/// `eludite.browser.tabs`' engine row, as an agent reads it.
fn tabs_engine(w: &Ws) -> Value {
    w.agent_call(cmds::TABS, json!({})).unwrap()["engine"].clone()
}

/// Brief 0039: the engine and CEF are searched for on first use, never at startup.
#[gpui::test]
fn nothing_is_searched_for_at_startup(cx: &mut gpui::TestAppContext) {
    let w = setup(cx);
    w.vcx.run_until_parked();
    assert!(!w.shell.read_with(&w.vcx, |s, _| s.browser().searched()));
    // The first question about the engine searches (a few file checks on this machine; nothing starts).
    let t = Instant::now();
    w.shell.read_with(&w.vcx, |s, _| s.browser().status());
    eprintln!("the first engine search took {:?}", t.elapsed());
    assert!(w.shell.read_with(&w.vcx, |s, _| s.browser().searched()));
    assert!(!w.shell.read_with(&w.vcx, |s, _| s.browser().started()));
}

/// Brief 0039: the engine's refusal shows the opt-in dialog with both remedies; declining (Escape) leaves the engine
/// off and the window showing the message; reopening the window is refused again without a second dialog (once per
/// workspace), and nothing was stored.
#[gpui::test]
fn a_refused_sandbox_offers_the_opt_in_once_and_declining_leaves_the_engine_off(
    cx: &mut gpui::TestAppContext,
) {
    let (mut w, seen) = setup_window(cx);
    w.shell
        .read_with(&w.vcx, |s, _| s.browser().set_no_sandbox_env(false));
    w.open_solution();
    seen.refuse_sandbox.store(true, Ordering::SeqCst);
    w.open_web_browser();
    w.wait("the sandbox dialog", |w| {
        w.browser_window(|b| b.sandbox_prompt().is_some())
    });
    let p = w.browser_window(|b| b.sandbox_prompt().cloned()).unwrap();
    assert!(!p.checked, "the opt-in is never preselected");
    let remedies = p.remedies();
    assert!(
        remedies.iter().any(|r| r.contains(
            "sudo chown root:root /opt/eludite/cef/chrome-sandbox && sudo chmod 4755 /opt/eludite/cef/chrome-sandbox"
        )),
        "{remedies:?}"
    );
    assert!(
        remedies.iter().any(|r| r.contains("user namespaces")),
        "{remedies:?}"
    );
    for id in [
        "web-browser-sandbox",
        "web-browser-sandbox-check",
        "web-browser-sandbox-ok",
        "web-browser-sandbox-cancel",
    ] {
        assert!(w.vcx.debug_bounds(id).is_some(), "{id} is drawn");
    }
    assert_eq!(*seen.attempts.lock().unwrap(), [false]);
    assert_eq!(seen.launches.load(Ordering::SeqCst), 0);

    // Escape is Cancel: the engine stays off, the window says why and how to change it.
    w.vcx.simulate_keystrokes("escape");
    w.vcx.run_until_parked();
    assert!(w.browser_window(|b| b.sandbox_prompt().is_none()));
    let message = w
        .browser_window(|b| b.message().map(str::to_owned))
        .unwrap();
    assert!(message.contains("sudo chmod 4755"), "{message}");
    assert!(
        message.contains("Tools > Options > Web Browser"),
        "{message}"
    );
    assert!(w.vcx.debug_bounds("web-browser-message").is_some());
    assert!(w.vcx.debug_bounds("web-browser-sandbox-strip").is_none());
    assert_eq!(seen.launches.load(Ordering::SeqCst), 0);
    assert_eq!(allow_setting(&w), (json!(false), "default".to_owned()));

    // Reopening the window tries again and is refused again, without a second dialog.
    w.controller.close_document(WEB_BROWSER);
    w.vcx.run_until_parked();
    w.open_web_browser();
    w.wait("a second attempt", |_| {
        seen.attempts.lock().unwrap().len() == 2
    });
    w.vcx.run_until_parked();
    assert!(w.browser_window(|b| b.sandbox_prompt().is_none()));
    assert!(w.browser_window(|b| b.message().is_some()));
    assert_eq!(*seen.attempts.lock().unwrap(), [false, false]);
    assert_eq!(seen.launches.load(Ordering::SeqCst), 0);
}

/// Brief 0039: Space checks "Run without the sandbox for this workspace" and Enter is OK: the setting is stored, the
/// engine starts again with the opt-in, the start is audited with `sandbox: none` and the strip shows; turning the
/// setting off again starts the next engine without the opt-in, which is refused again. Brief 0047: stored in the
/// person's state for the workspace (mode 0600), never the workspace's `.eludite/settings.json`; `settings.get` names
/// the source `user-workspace`; the audit entry and `tabs` say `allowed_by: dialog`.
#[gpui::test]
fn taking_the_opt_in_stores_it_restarts_audits_and_shows_the_strip(cx: &mut gpui::TestAppContext) {
    let (mut w, seen) = setup_window(cx);
    w.shell
        .read_with(&w.vcx, |s, _| s.browser().set_no_sandbox_env(false));
    w.open_solution();
    seen.refuse_sandbox.store(true, Ordering::SeqCst);
    w.open_web_browser();
    w.wait("the sandbox dialog", |w| {
        w.browser_window(|b| b.sandbox_prompt().is_some())
    });
    w.vcx.simulate_keystrokes("space");
    w.vcx.run_until_parked();
    assert!(w.browser_window(|b| b.sandbox_prompt().is_some_and(|p| p.checked)));
    w.vcx.simulate_keystrokes("enter");
    w.wait(
        "the engine started without the sandbox, the strip and the tab",
        |w| w.browser_window(|b| b.running_without_sandbox()) && w.strip().len() == 1,
    );
    assert!(w.browser_window(|b| b.sandbox_prompt().is_none() && b.message().is_none()));
    assert_eq!(*seen.attempts.lock().unwrap(), [false, true]);
    assert_eq!(seen.launches.load(Ordering::SeqCst), 1);
    assert!(w.vcx.debug_bounds("web-browser-sandbox-strip").is_some());
    // Stored in the person's state for the workspace, as the person's audited command; the workspace's file is not
    // written.
    w.wait("the setting stored", |w| {
        allow_setting(w) == (json!(true), "user-workspace".to_owned())
    });
    let state = state_file(&mut w);
    assert!(
        state.starts_with(w.path("user-config/workspaces")),
        "{}",
        state.display()
    );
    w.wait("the state file written", |_| {
        std::fs::read_to_string(&state)
            .is_ok_and(|t| t.contains("\"browser.allowNoSandbox\": true"))
    });
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let mode = std::fs::metadata(&state).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "the state file is the person's alone");
    }
    assert!(
        !w.path(".eludite/settings.json").exists(),
        "the workspace's file is never written"
    );
    assert!(
        w.audit()
            .iter()
            .any(|c| c == eludite_commands::settings::SET)
    );
    assert_eq!(
        engine_starts(&w),
        [json!({"sandbox": "none", "allowed_by": "dialog"})]
    );
    let engine = tabs_engine(&w);
    assert_eq!(
        (&engine["sandbox"], &engine["allowed_by"]),
        (&json!("none"), &json!("dialog")),
        "{engine}"
    );

    // The setting off again (as Tools > Options writes it, in the person's state by default): the next start
    // carries no opt-in.
    w.agent_invoke(
        eludite_commands::settings::SET,
        json!({"key": "browser.allowNoSandbox", "value": false}),
    )
    .unwrap();
    w.wait("the setting applied", |w| {
        w.shell
            .read_with(&w.vcx, |s, _| !s.browser().settings().allow_no_sandbox)
    });
    let closed = w.shell.read_with(&w.vcx, |s, _| s.browser().shutdown());
    closed.recv_timeout(Duration::from_secs(10)).unwrap();
    w.wait("the strip gone with the engine", |w| {
        !w.browser_window(|b| b.running_without_sandbox())
    });
    assert!(w.vcx.debug_bounds("web-browser-sandbox-strip").is_none());
    w.with_window(|b, cx| b.new_tab(cx));
    w.wait("a third attempt", |_| {
        seen.attempts.lock().unwrap().len() == 3
    });
    assert_eq!(*seen.attempts.lock().unwrap(), [false, true, false]);
    assert_eq!(seen.launches.load(Ordering::SeqCst), 1, "refused again");
}

/// Brief 0047: a `true` in the workspace's `.eludite/settings.json` (a cloned repository's) is ignored: the engine
/// starts without the opt-in and is refused, the dialog still comes, the window and the Output window warn, and
/// `settings.get` reports the key ignored with the default as the value. Accept then writes the person's state, not
/// the workspace's file, and the warning stays while that file carries the key; removing it there clears it.
#[gpui::test]
fn a_workspace_file_carrying_the_opt_in_is_ignored_with_a_warning_and_the_dialog_still_appears(
    cx: &mut gpui::TestAppContext,
) {
    let (mut w, seen) = setup_window(cx);
    w.shell
        .read_with(&w.vcx, |s, _| s.browser().set_no_sandbox_env(false));
    let workspace_file = w.path(".eludite/settings.json");
    std::fs::create_dir_all(workspace_file.parent().unwrap()).unwrap();
    std::fs::write(
        &workspace_file,
        r#"{"browser.allowNoSandbox": true, "browser.homePage": "about:blank"}"#,
    )
    .unwrap();
    w.open_solution();
    w.wait("the warning", |w| w.browser_window(|b| b.opt_in_ignored()));
    let out = settings_get(&w);
    assert_eq!(out["ignored_keys"], json!(["browser.allowNoSandbox"]));
    assert_eq!(allow_setting(&w), (json!(false), "default".to_owned()));
    assert!(
        !w.shell
            .read_with(&w.vcx, |s, _| s.browser().settings().allow_no_sandbox)
    );
    assert!(
        browser_output(&w).contains(super::browser_window::OPT_IN_IGNORED),
        "{}",
        browser_output(&w)
    );
    assert_eq!(
        super::browser_window::OPT_IN_IGNORED,
        "browser.allowNoSandbox in .eludite/settings.json is ignored: the sandbox opt-in is per person"
    );

    // The engine starts without the opt-in, is refused, and the dialog comes; the warning shows in the window.
    seen.refuse_sandbox.store(true, Ordering::SeqCst);
    w.open_web_browser();
    w.wait("the sandbox dialog", |w| {
        w.browser_window(|b| b.sandbox_prompt().is_some())
    });
    assert_eq!(*seen.attempts.lock().unwrap(), [false]);
    assert!(w.vcx.debug_bounds("web-browser-opt-in-ignored").is_some());

    // Accept: the person's state, not the workspace's file.
    w.vcx.simulate_keystrokes("space enter");
    w.wait("the engine started without the sandbox", |w| {
        w.browser_window(|b| b.running_without_sandbox())
    });
    w.wait("the setting stored", |w| {
        allow_setting(w) == (json!(true), "user-workspace".to_owned())
    });
    let state = state_file(&mut w);
    w.wait("the state file written", |_| state.exists());
    let workspace_text = std::fs::read_to_string(&workspace_file).unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(&workspace_text).unwrap(),
        json!({"browser.allowNoSandbox": true, "browser.homePage": "about:blank"}),
        "the workspace's file is untouched"
    );
    assert!(
        w.browser_window(|b| b.opt_in_ignored()),
        "the warning stays while the file carries the key"
    );
    assert_eq!(
        engine_starts(&w),
        [json!({"sandbox": "none", "allowed_by": "dialog"})]
    );

    // Removing the key from the workspace's file (null, the one write allowed there) clears the warning.
    w.agent_invoke(
        eludite_commands::settings::SET,
        json!({"key": "browser.allowNoSandbox", "value": null, "scope": "solution"}),
    )
    .unwrap();
    w.wait("the warning gone", |w| {
        !w.browser_window(|b| b.opt_in_ignored())
    });
    assert!(w.vcx.debug_bounds("web-browser-opt-in-ignored").is_none());
    assert!(settings_get(&w).get("ignored_keys").is_none());
    // A value there is refused, as it would be ignored.
    assert!(
        w.agent_invoke(
            eludite_commands::settings::SET,
            json!({"key": "browser.allowNoSandbox", "value": true, "scope": "solution"}),
        )
        .is_err()
    );
}

/// Brief 0047: the person's stored answer (their state for the workspace, from an earlier session) starts the engine
/// without the sandbox and with no dialog; the audit entry and `tabs` say `allowed_by: options`.
#[gpui::test]
fn a_stored_answer_starts_the_engine_without_a_dialog_and_the_audit_names_options(
    cx: &mut gpui::TestAppContext,
) {
    let (mut w, seen) = setup_window(cx);
    w.shell
        .read_with(&w.vcx, |s, _| s.browser().set_no_sandbox_env(false));
    w.open_solution();
    let state = state_file(&mut w);
    std::fs::create_dir_all(state.parent().unwrap()).unwrap();
    std::fs::write(&state, r#"{"browser.allowNoSandbox": true}"#).unwrap();
    w.wait("the stored answer read", |w| {
        w.shell
            .read_with(&w.vcx, |s, _| s.browser().settings().allow_no_sandbox)
    });
    assert_eq!(
        allow_setting(&w),
        (json!(true), "user-workspace".to_owned())
    );
    seen.refuse_sandbox.store(true, Ordering::SeqCst);
    w.open_web_browser();
    w.wait("the engine started without the sandbox", |w| {
        w.browser_window(|b| b.running_without_sandbox()) && w.strip().len() == 1
    });
    assert!(w.browser_window(|b| b.sandbox_prompt().is_none()));
    assert_eq!(*seen.attempts.lock().unwrap(), [true]);
    assert_eq!(
        engine_starts(&w),
        [json!({"sandbox": "none", "allowed_by": "options"})]
    );
    assert_eq!(tabs_engine(&w)["allowed_by"], "options");
}

/// Brief 0047: `ELUDITE_CHROME_NO_SANDBOX=1` still lets the engine run without the sandbox, with no dialog and nothing
/// stored; the audit entry and `tabs` say `allowed_by: variable`.
#[gpui::test]
fn the_variable_still_allows_it_and_the_audit_names_it(cx: &mut gpui::TestAppContext) {
    let (mut w, seen) = setup_window(cx);
    w.shell
        .read_with(&w.vcx, |s, _| s.browser().set_no_sandbox_env(true));
    w.open_solution();
    seen.refuse_sandbox.store(true, Ordering::SeqCst);
    w.open_web_browser();
    w.wait("the engine started without the sandbox", |w| {
        w.browser_window(|b| b.running_without_sandbox()) && w.strip().len() == 1
    });
    assert!(w.browser_window(|b| b.sandbox_prompt().is_none()));
    assert_eq!(*seen.attempts.lock().unwrap(), [true]);
    assert_eq!(
        engine_starts(&w),
        [json!({"sandbox": "none", "allowed_by": "variable"})]
    );
    assert_eq!(tabs_engine(&w)["allowed_by"], "variable");
    assert_eq!(allow_setting(&w), (json!(false), "default".to_owned()));
    assert!(!state_file(&mut w).exists());
}
