//! Headless tests of the Web Browser window (brief 0032) against a fake engine that stands for the embedded one: it
//! answers the commands as an [`Engine`] and tells the window what `eludite-chromium` would ([`WindowEvent`]s through
//! [`super::browser::BrowserBus::window_sink`]), with a [`PageDriver`] that records what the window sends the tabs.
//!
//! The window opens from View > Other Windows > Web Browser with a tab; the address bar navigates through the bus;
//! Back and Forward follow the history; the tab strip follows `tabs`; Ctrl+T and Ctrl+W; the "Agent is driving" strip
//! while a fake agent's call is in flight, its Stop and the person's click interrupting the agent's `wait`, and the
//! agent's next action refused until it reads `tabs`; a dialog and a permission request from the engine as shell
//! dialogs whose answers reach the engine; the context menu; DevTools as a tab; downloads' Output line; closing the
//! window (the engine lingers) and the workspace (the engine closes); and the engine-missing message.

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
}

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
}

struct FakeEmbedded {
    seen: Arc<Seen>,
    running: bool,
}

impl Engine for FakeEmbedded {
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
        self.seen.launches.fetch_add(1, Ordering::SeqCst);
        self.seen
            .tell(WindowEvent::Started(Arc::new(Driver(self.seen.clone()))));
        Ok(Some(LaunchInfo::default()))
    }
    fn info(&self) -> Option<LaunchInfo> {
        self.running.then(LaunchInfo::default)
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
            .set_engine_factory(Arc::new(move |_config, _log: LogSink| {
                Box::new(FakeEmbedded {
                    seen: engines.clone(),
                    running: false,
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
    assert!(
        seen.sent
            .lock()
            .unwrap()
            .iter()
            .any(|(m, p)| m == "Page.navigate" && p["url"] == "http://localhost:5000/orders")
    );
    assert!(w.audit().iter().any(|c| c == cmds::NAVIGATE));
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
    assert!(t0.elapsed() < Duration::from_secs(3), "{:?}", t0.elapsed());
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
    w.click("web-browser-tab-close-1");
    w.wait("DevTools closed", |w| w.strip().len() == 1);
    assert_eq!(
        seen.closed.lock().unwrap().as_slice(),
        [format!("dt{target}")]
    );

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
    let empty = tempfile::tempdir().unwrap();
    w.shell.read_with(&w.vcx, |s, _| {
        s.browser().set_chromium_search(ChromiumSearch {
            engine: None,
            engine_dirs: vec![empty.path().to_path_buf()],
            cef: Vec::new(),
            cef_cache: None,
        })
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
