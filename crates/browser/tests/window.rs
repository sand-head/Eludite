//! Brief 0032's commands on a [`Browser`] over a fake engine (no browser needed): `record` makes a GIF with one
//! frame per tick from the engine's frames, `dialog` answers the engine's dialog and `input` reports the dialog it
//! opened instead of waiting on the paused page (and reports a stop in the debugger the same way, brief 0038), `devtools` reaches the engine, `navigate` stops, and the person's
//! [`eludite_browser::Interrupt`] ends a `wait` with `interrupted_by: "user"` and fails a stopped call.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

use eludite_browser::embedded::Frame;
use eludite_browser::{
    Browser, CdpEvent, DebuggerPauses, DialogAnswer, Engine, EngineConfig, EngineError,
    FrameSource, LaunchInfo, PendingDialog, TargetInfo,
};
use eludite_commands::browser as cmds;
use serde_json::{Value, json};

/// Frames that change on every look.
#[derive(Default)]
struct Moving(AtomicU64);

impl FrameSource for Moving {
    fn sequence(&self) -> u64 {
        self.0.fetch_add(1, Ordering::SeqCst) + 1
    }

    fn read(&self, _consume: bool, f: &mut dyn FnMut(&Frame<'_>)) -> bool {
        let seq = self.0.load(Ordering::SeqCst);
        let pixels = vec![(seq % 250) as u8; 16 * 8 * 4];
        f(&Frame {
            sequence: seq,
            width: 16,
            height: 8,
            stride: 64,
            pixels: &pixels,
            dirty: &[],
            paint_ns: 0,
            copy_ns: 0,
        });
        true
    }

    fn set_listener(&self, _f: Option<Box<dyn Fn() + Send + Sync>>) {}
}

/// A tab's DevTools opened, at a point or not.
type Inspected = (String, Option<(f64, f64)>);

#[derive(Default)]
struct Seen {
    sent: Mutex<Vec<String>>,
    dialog: Mutex<Option<PendingDialog>>,
    answers: Mutex<Vec<DialogAnswer>>,
    devtools: Mutex<Vec<Inspected>>,
    /// A mouse press opens this dialog (and is then never answered, as a paused page does).
    press_opens: Mutex<Option<PendingDialog>>,
    /// A mouse press stops the page in the debugger: the shell's debugger marks the tab in these (brief 0038).
    press_pauses: Mutex<Option<Arc<DebuggerPauses>>>,
}

struct Fake {
    seen: Arc<Seen>,
    frames: Arc<Moving>,
    running: bool,
}

impl Engine for Fake {
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
        Some(LaunchInfo::default())
    }
    fn targets(&self) -> Result<Vec<TargetInfo>, EngineError> {
        Ok(vec![TargetInfo {
            target_id: "1".into(),
            url: "about:blank".into(),
            title: String::new(),
        }])
    }
    fn open_tab(&mut self, _url: &str) -> Result<String, EngineError> {
        Ok("1".into())
    }
    fn close_tab(&mut self, _t: &str) -> Result<(), EngineError> {
        Ok(())
    }
    fn activate_tab(&mut self, _t: &str) -> Result<(), EngineError> {
        Ok(())
    }
    fn attach(&mut self, t: &str) -> Result<String, EngineError> {
        Ok(t.to_owned())
    }
    fn send(
        &self,
        _s: &str,
        method: &str,
        params: Value,
        _t: Duration,
    ) -> Result<Value, EngineError> {
        self.seen.sent.lock().unwrap().push(method.to_owned());
        Ok(match method {
            "Runtime.evaluate" => {
                let e = params["expression"].as_str().unwrap_or_default();
                if e.contains("location.href") {
                    json!({"result": {"type": "object", "value": {"u": "about:blank", "t": ""}}})
                } else {
                    json!({"result": {"type": "boolean", "value": false}})
                }
            }
            "Page.getNavigationHistory" => {
                json!({"currentIndex": 0, "entries": [{"id": 1, "url": "about:blank", "userTypedURL": "about:blank", "title": "", "transitionType": "typed"}]})
            }
            "DOM.getNodeForLocation" => json!({"backendNodeId": 5, "frameId": "F"}),
            _ => json!({}),
        })
    }
    fn send_many_unless(
        &self,
        session: &str,
        calls: Vec<(String, Value)>,
        timeout: Duration,
        give_up: &dyn Fn() -> bool,
    ) -> Vec<Result<Value, EngineError>> {
        calls
            .into_iter()
            .map(|(m, p)| {
                if m == "Input.dispatchMouseEvent" && p["type"] == "mousePressed" {
                    let opens = self.seen.press_opens.lock().unwrap().clone();
                    if let Some(d) = opens {
                        *self.seen.dialog.lock().unwrap() = Some(d);
                    }
                }
                if m == "Input.dispatchMouseEvent"
                    && p["type"] == "mousePressed"
                    && let Some(pauses) = self.seen.press_pauses.lock().unwrap().as_ref()
                {
                    let seen = self.seen.clone();
                    let pauses = pauses.clone();
                    // The debugger learns of the stop a moment later, as the shell does from the adapter.
                    std::thread::spawn(move || {
                        std::thread::sleep(Duration::from_millis(50));
                        pauses.set(vec!["t1".into()]);
                        seen.sent.lock().unwrap().push("paused".into());
                    });
                    let t0 = Instant::now();
                    while !give_up() {
                        assert!(t0.elapsed() < Duration::from_secs(5), "never gave up");
                        std::thread::sleep(Duration::from_millis(5));
                    }
                    return Err(EngineError::Interrupted);
                }
                if self.seen.dialog.lock().unwrap().is_some() {
                    // The paused page answers nothing; the caller gives up.
                    let t0 = Instant::now();
                    while !give_up() {
                        assert!(t0.elapsed() < Duration::from_secs(5), "never gave up");
                        std::thread::sleep(Duration::from_millis(5));
                    }
                    return Err(EngineError::Interrupted);
                }
                self.send(session, &m, p, timeout)
            })
            .collect()
    }
    fn subscribe(&self, _s: &str) -> Result<mpsc::Receiver<CdpEvent>, EngineError> {
        let (tx, rx) = mpsc::channel();
        std::mem::forget(tx);
        Ok(rx)
    }
    fn frames(&self, _t: &str) -> Option<Arc<dyn FrameSource>> {
        Some(self.frames.clone())
    }
    fn pending_dialog(&self, _t: &str) -> Option<PendingDialog> {
        self.seen.dialog.lock().unwrap().clone()
    }
    fn answer_dialog(&self, _t: &str, answer: &DialogAnswer) -> Result<PendingDialog, EngineError> {
        self.seen.answers.lock().unwrap().push(answer.clone());
        self.seen
            .dialog
            .lock()
            .unwrap()
            .take()
            .ok_or_else(|| EngineError::Launch("no dialog".into()))
    }
    fn devtools(&mut self, t: &str, inspect: Option<(f64, f64)>) -> Result<bool, EngineError> {
        let mut d = self.seen.devtools.lock().unwrap();
        d.push((t.to_owned(), inspect));
        Ok(d.len() == 1)
    }
    fn shutdown(&mut self) {
        self.running = false;
    }
}

fn browser() -> (Browser, Arc<Seen>) {
    let seen = Arc::new(Seen::default());
    let engine = Fake {
        seen: seen.clone(),
        frames: Arc::default(),
        running: false,
    };
    let mut b = Browser::new(Box::new(engine), Arc::new(|_: &str| {}));
    b.apply(cmds::parse(cmds::TAB_OPEN, json!({})).unwrap())
        .unwrap();
    (b, seen)
}

fn call(b: &mut Browser, id: &str, input: Value) -> Result<Value, String> {
    b.apply(cmds::parse(id, input).map_err(|e| e.to_string())?)
        .map(|o| o.to_json())
        .map_err(|e| e.to_string())
}

#[test]
fn record_makes_a_gif_with_one_frame_per_tick_from_the_engines_frames() {
    let (mut b, _) = browser();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("flow.gif");
    let started = call(
        &mut b,
        cmds::RECORD,
        json!({"action": "start", "path": path, "fps": 20, "max_seconds": 1}),
    )
    .unwrap();
    assert_eq!(started["recording"], true);
    assert_eq!(started["fps"], 20);
    let again = call(
        &mut b,
        cmds::RECORD,
        json!({"action": "start", "path": path}),
    )
    .unwrap_err();
    assert!(again.contains("stop it first"), "{again}");
    std::thread::sleep(Duration::from_millis(1200));
    let done = call(&mut b, cmds::RECORD, json!({"action": "stop"})).unwrap();
    assert_eq!(done["frames"], 20, "{done}");
    assert_eq!(done["duration_ms"], 1000.0);
    assert_eq!(done["stopped_by"], "max_seconds");
    assert_eq!(
        (done["width"].as_u64(), done["height"].as_u64()),
        (Some(16), Some(8))
    );
    assert!(
        done["thumbnail"]
            .as_str()
            .is_some_and(|t| t.starts_with("iVBOR")),
        "a PNG"
    );
    let gif = std::fs::read(&path).unwrap();
    assert!(gif.starts_with(b"GIF89a"));
    assert_eq!(done["bytes"], gif.len());
    use image::AnimationDecoder;
    let frames = image::codecs::gif::GifDecoder::new(std::io::Cursor::new(gif))
        .unwrap()
        .into_frames()
        .count();
    assert_eq!(frames, 20);
    assert!(call(&mut b, cmds::RECORD, json!({"action": "stop"})).is_err());
}

#[test]
fn input_reports_the_dialog_it_opened_and_dialog_answers_it() {
    let (mut b, seen) = browser();
    *seen.press_opens.lock().unwrap() = Some(PendingDialog {
        id: 7,
        kind: "confirm".into(),
        message: "Delete it?".into(),
        default_text: None,
    });
    let t0 = Instant::now();
    let out = call(
        &mut b,
        cmds::INPUT,
        json!({"action": "click", "x": 10, "y": 10, "wait_ms": 3000}),
    )
    .unwrap();
    assert!(
        t0.elapsed() < Duration::from_secs(2),
        "no wait on a paused page: {:?}",
        t0.elapsed()
    );
    assert_eq!(
        out["dialog"],
        json!({"kind": "confirm", "message": "Delete it?"})
    );
    let answered = call(&mut b, cmds::DIALOG, json!({"action": "dismiss"})).unwrap();
    assert_eq!(
        answered,
        json!({"tab": "t1", "kind": "confirm", "message": "Delete it?", "action": "dismiss"})
    );
    assert!(!seen.answers.lock().unwrap()[0].accept);
    let none = call(&mut b, cmds::DIALOG, json!({"action": "accept"})).unwrap_err();
    assert!(none.contains("no dialog is open"), "{none}");
    // A prompt answered with text.
    *seen.dialog.lock().unwrap() = Some(PendingDialog {
        id: 8,
        kind: "prompt".into(),
        message: "Name?".into(),
        default_text: Some("x".into()),
    });
    let p = call(
        &mut b,
        cmds::DIALOG,
        json!({"action": "accept", "text": "Ada"}),
    )
    .unwrap();
    assert_eq!(p["text"], "Ada");
}

#[test]
fn input_reports_a_stop_in_the_debugger_instead_of_waiting_on_the_paused_page() {
    let (mut b, seen) = browser();
    let pauses = Arc::new(DebuggerPauses::default());
    b.set_pauses(pauses.clone());
    *seen.press_pauses.lock().unwrap() = Some(pauses.clone());
    let t0 = Instant::now();
    let out = call(
        &mut b,
        cmds::INPUT,
        json!({"action": "click", "x": 10, "y": 10, "wait_ms": 3000}),
    )
    .unwrap();
    assert!(
        t0.elapsed() < Duration::from_secs(2),
        "no wait on a page stopped in the debugger: {:?}",
        t0.elapsed()
    );
    assert_eq!(out["paused"], true, "{out}");
    assert!(out.get("dialog").is_none(), "{out}");
    // The session continued: the next input is answered and says nothing of a pause.
    *seen.press_pauses.lock().unwrap() = None;
    assert!(pauses.set(Vec::new()));
    assert!(!pauses.set(Vec::new()), "no change, no news");
    let out = call(
        &mut b,
        cmds::INPUT,
        json!({"action": "click", "x": 10, "y": 10}),
    )
    .unwrap();
    assert!(out.get("paused").is_none(), "{out}");
}

#[test]
fn devtools_stop_and_the_persons_hand() {
    let (mut b, seen) = browser();
    let d = call(&mut b, cmds::DEVTOOLS, json!({"inspect": {"x": 3, "y": 4}})).unwrap();
    assert_eq!(d, json!({"tab": "t1", "opened": true}));
    assert_eq!(
        seen.devtools.lock().unwrap()[0],
        ("1".to_owned(), Some((3., 4.)))
    );
    assert_eq!(
        call(&mut b, cmds::DEVTOOLS, json!({})).unwrap()["opened"],
        false
    );
    call(&mut b, cmds::NAVIGATE, json!({"action": "stop"})).unwrap();
    assert!(
        seen.sent
            .lock()
            .unwrap()
            .iter()
            .any(|m| m == "Page.stopLoading")
    );
    // The tabs say where history can go.
    let tabs = call(&mut b, cmds::TABS, json!({})).unwrap();
    assert_eq!(tabs["tabs"][0]["can_go_back"], false);

    // A wait the person interrupts ends at once with interrupted_by.
    let interrupt = b.interrupt();
    let i = interrupt.clone();
    let hand = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(200));
        i.took_over();
    });
    let t0 = Instant::now();
    let w = call(
        &mut b,
        cmds::WAIT,
        json!({"for": "selector", "css": "#never", "wait_ms": 10000}),
    )
    .unwrap();
    hand.join().unwrap();
    let took = t0.elapsed();
    assert!(took < Duration::from_millis(1500), "{took:?}");
    assert_eq!(w["interrupted_by"], "user", "{w}");
    assert_eq!(w["timeout"], false);
    assert!(w.get("satisfied").is_none());
    eprintln!(
        "the person's hand ended a wait {:?} after it",
        took - Duration::from_millis(200)
    );
    // The next wait runs normally (the counters are per call).
    let w = call(
        &mut b,
        cmds::WAIT,
        json!({"for": "selector", "css": "#never", "wait_ms": 100}),
    )
    .unwrap();
    assert_eq!(w["timeout"], true);
    assert!(w.get("interrupted_by").is_none());
}
