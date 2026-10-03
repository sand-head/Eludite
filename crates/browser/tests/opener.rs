//! `eludite.browser.open_external` with a fake opener (brief 0024): `ELUDITE_OPENER` names a script that records the
//! url it is given. The browser is a fake engine with one tab (no Chrome needed), so the default url is the tab's.
//! This binary has this one test, so setting the variable races no other thread.

use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

use eludite_browser::{
    Browser, CdpEvent, Engine, EngineConfig, EngineError, LaunchInfo, TargetInfo,
};
use eludite_commands::browser::{self as cmds, BrowserOutput};
use serde_json::{Value, json};

/// A browser with one blank tab at `PAGE`.
struct OneTab {
    running: bool,
}

const PAGE: &str = "http://127.0.0.1:1/page";

impl Engine for OneTab {
    fn name(&self) -> &'static str {
        "one-tab"
    }
    fn configure(&mut self, _: EngineConfig) {}
    fn is_running(&self) -> bool {
        self.running
    }
    fn launch(&mut self) -> Result<Option<LaunchInfo>, EngineError> {
        let was = self.running;
        self.running = true;
        Ok((!was).then(LaunchInfo::default))
    }
    fn info(&self) -> Option<LaunchInfo> {
        None
    }
    fn targets(&self) -> Result<Vec<TargetInfo>, EngineError> {
        Ok(vec![TargetInfo {
            target_id: "T1".into(),
            url: "about:blank".into(),
            title: String::new(),
        }])
    }
    fn open_tab(&mut self, _: &str) -> Result<String, EngineError> {
        Ok("T1".into())
    }
    fn close_tab(&mut self, _: &str) -> Result<(), EngineError> {
        Ok(())
    }
    fn activate_tab(&mut self, _: &str) -> Result<(), EngineError> {
        Ok(())
    }
    fn attach(&mut self, _: &str) -> Result<String, EngineError> {
        Ok("S1".into())
    }
    fn send(&self, _: &str, method: &str, _: Value, _: Duration) -> Result<Value, EngineError> {
        Ok(match method {
            "Page.getFrameTree" => {
                json!({"frameTree": {"frame": {"id": "F", "loaderId": "L", "url": PAGE}}})
            }
            _ => json!({}),
        })
    }
    fn subscribe(&self, _: &str) -> Result<mpsc::Receiver<CdpEvent>, EngineError> {
        let (tx, rx) = mpsc::channel();
        // Kept open, as a live tab's are (dropped with the engine).
        std::mem::forget(tx);
        Ok(rx)
    }
    fn shutdown(&mut self) {
        self.running = false;
    }
}

/// Writes an opener script that records its argument, then lingers: open_external must not wait for it.
#[cfg(unix)]
fn fake_opener(dir: &std::path::Path, record: &std::path::Path) -> std::path::PathBuf {
    let script = dir.join("opener.sh");
    std::fs::write(
        &script,
        format!(
            "#!/bin/sh\nprintf '%s\\n' \"$1\" >> '{}'\nsleep 2\n",
            record.display()
        ),
    )
    .unwrap();
    std::process::Command::new("chmod")
        .args(["+x", &script.to_string_lossy()])
        .status()
        .unwrap();
    script
}

/// Windows: a batch file; `ping` is the wait that needs no console input.
#[cfg(windows)]
fn fake_opener(dir: &std::path::Path, record: &std::path::Path) -> std::path::PathBuf {
    let script = dir.join("opener.cmd");
    std::fs::write(
        &script,
        format!(
            "@echo off\r\n>>\"{}\" echo %~1\r\nping -n 3 127.0.0.1 >nul\r\n",
            record.display()
        ),
    )
    .unwrap();
    script
}

#[test]
fn open_external_runs_the_opener_and_never_waits() {
    let dir = tempfile::tempdir().unwrap();
    let record = dir.path().join("opened.txt");
    let script = fake_opener(dir.path(), &record);
    // SAFETY: this test binary runs this one test; no other thread reads the environment meanwhile.
    #[allow(unsafe_code)]
    unsafe {
        std::env::set_var(eludite_browser::browser::OPENER_ENV, &script);
    }
    let mut b = Browser::new(Box::new(OneTab { running: false }), Arc::new(|_: &str| {}));
    let open =
        |b: &mut Browser, input: Value| b.apply(cmds::parse(cmds::OPEN_EXTERNAL, input).unwrap());
    // No url and no tab: nothing to open.
    let err = open(&mut b, json!({})).unwrap_err().to_string();
    assert!(err.contains("give `url`"), "{err}");
    let t = Instant::now();
    let out = open(&mut b, json!({"url": "https://example.com/a?b=1"})).unwrap();
    assert!(
        t.elapsed() < Duration::from_millis(1000),
        "it did not wait for the opener"
    );
    let BrowserOutput::OpenExternal(o) = out else {
        panic!("{out:?}")
    };
    assert_eq!(o.url, "https://example.com/a?b=1");
    assert_eq!(
        o.command,
        format!("{} https://example.com/a?b=1", script.display())
    );
    // The active tab's url is the default.
    b.apply(cmds::parse(cmds::TAB_OPEN, json!({})).unwrap())
        .unwrap();
    let BrowserOutput::OpenExternal(o) = open(&mut b, json!({})).unwrap() else {
        panic!()
    };
    assert_eq!(o.url, PAGE);
    let until = Instant::now() + Duration::from_secs(5);
    loop {
        let text = std::fs::read_to_string(&record)
            .unwrap_or_default()
            .replace("\r\n", "\n");
        if text.lines().count() == 2 {
            // Neither opener is waited for, so they may record in either order.
            let mut opened: Vec<&str> = text.lines().collect();
            opened.sort_unstable();
            assert_eq!(
                opened,
                ["http://127.0.0.1:1/page", "https://example.com/a?b=1"]
            );
            break;
        }
        assert!(Instant::now() < until, "the opener recorded {text:?}");
        std::thread::sleep(Duration::from_millis(20));
    }
    // A missing opener fails the command, naming it.
    #[allow(unsafe_code)]
    unsafe {
        std::env::set_var(eludite_browser::browser::OPENER_ENV, "/nonexistent/opener");
    }
    let err = open(&mut b, json!({"url": "http://localhost/"}))
        .unwrap_err()
        .to_string();
    assert!(err.contains("/nonexistent/opener"), "{err}");
}
