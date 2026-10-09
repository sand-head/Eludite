//! Brief 0023's browser commands against the embedded engine (brief 0031): `eludite-chromium` (CEF, out of process,
//! frames in shared memory) as the [`Engine`] of an unchanged [`Browser`]. The subset the brief names: `tab_open`,
//! `navigate`, `screenshot` (from the frame ring, checked pixel by pixel; `Page.captureScreenshot` for a full page),
//! `read_page` and `evaluate`, then `tab_close`, and the engine killed and relaunched by the next command.
//! Brief 0032: every key of the Web Browser window's table reaching the page with its key code, DOM key and DOM
//! code; an agent's click opening a `confirm` answered by `dialog`; `devtools` as a tab `tabs` does not list;
//! `record` from the ring; and `input` click to the next frame in the ring (the 50 ms p95 budget, asserted on a
//! quiet machine).
//!
//! Skips with a message when the engine is not built with CEF (`tools/cef/fetch.sh`, then `CEF_PATH=...
//! cargo build -p eludite-chromium --features eludite-chromium/cef`) or off Linux. As root the engine gets
//! `--allow-no-sandbox` (brief 0039; Chromium does not sandbox root), and `tabs` then reports `engine.sandbox: none`.
//! Timings print with `-- --nocapture`.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use eludite_browser::{Browser, ChromiumSearch, EmbeddedChromium, EngineConfig};
use eludite_commands::CommandError;
use eludite_commands::browser as cmds;
use serde_json::{Value, json};

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

/// Serve `tests/fixtures/` on 127.0.0.1 (as tests/chrome.rs does).
fn serve() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            std::thread::spawn(move || answer(stream));
        }
    });
    format!("http://127.0.0.1:{port}")
}

fn answer(mut stream: TcpStream) {
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    let mut request_line = String::new();
    if reader.read_line(&mut request_line).is_err() {
        return;
    }
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" || line == "\n" {
            break;
        }
    }
    let path = request_line.split_whitespace().nth(1).unwrap_or("/");
    let path = path
        .split(['?', '#'])
        .next()
        .unwrap_or("/")
        .trim_start_matches('/');
    let file = fixtures().join(path);
    let (status, body, mime) = if !path.is_empty() && !path.contains("..") && file.is_file() {
        let mime = if path.ends_with(".html") {
            "text/html; charset=utf-8"
        } else {
            "application/octet-stream"
        };
        ("200 OK", std::fs::read(&file).unwrap(), mime)
    } else {
        ("404 Not Found", b"not found".to_vec(), "text/plain")
    };
    let head = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {mime}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n",
        body.len()
    );
    let _ = stream.write_all(head.as_bytes());
    let _ = stream.write_all(&body);
    let _ = stream.flush();
    let _ = stream.read(&mut [0u8; 1]);
}

fn running_as_root() -> bool {
    std::process::Command::new("id")
        .arg("-u")
        .output()
        .is_ok_and(|o| String::from_utf8_lossy(&o.stdout).trim() == "0")
}

fn p95(mut v: Vec<f64>) -> (f64, f64, f64) {
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let at = |q: f64| v[((v.len() as f64 * q).ceil() as usize).clamp(1, v.len()) - 1];
    (at(0.5), at(0.95), *v.last().unwrap())
}

fn decode(b64: &str) -> image::RgbaImage {
    const A: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut bytes = Vec::new();
    let mut acc = 0u32;
    let mut bits = 0;
    for c in b64.bytes().filter(|c| *c != b'=') {
        acc = acc << 6 | A.iter().position(|a| *a == c).unwrap() as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            bytes.push((acc >> bits) as u8);
        }
    }
    image::load_from_memory(&bytes).unwrap().to_rgba8()
}

struct Run {
    browser: Browser,
    log: Arc<Mutex<Vec<String>>>,
}

impl Run {
    fn call(&mut self, id: &str, input: Value) -> Result<Value, CommandError> {
        let request = cmds::parse(id, input)?;
        self.browser.apply(request).map(|o| o.to_json())
    }

    fn ok(&mut self, id: &str, input: Value) -> Value {
        match self.call(id, input.clone()) {
            Ok(v) => v,
            Err(e) => panic!("{id} {input}: {e}\nlog: {:#?}", self.log.lock().unwrap()),
        }
    }
}

const QUADRANTS: &str = "data:text/html,<html><head><title>q</title><style>html,body{margin:0;height:100%25;overflow:hidden}\
    div{position:absolute;width:50%25;height:50%25}</style></head><body>\
    <div style='left:0;top:0;background:%23f00'></div><div style='right:0;top:0;background:%23008000'></div>\
    <div style='left:0;bottom:0;background:%2300f'></div><div style='right:0;bottom:0;background:%23000'></div></body></html>";

#[test]
fn the_commands_against_the_embedded_engine() {
    if !cfg!(target_os = "linux") {
        println!("SKIPPED: the embedded engine runs on Linux only so far (brief 0031)");
        return;
    }
    let search = ChromiumSearch::defaults();
    if let Err(why) = search.find_engine().and_then(|e| search.find_cef(&e)) {
        eludite_test_support::skip("cef", why);
        return;
    }
    let profile = tempfile::tempdir().unwrap();
    let log: Arc<Mutex<Vec<String>>> = Arc::default();
    let sink = log.clone();
    let engine = EmbeddedChromium::new(
        EngineConfig {
            executable: None,
            profile_dir: profile.path().join(".eludite/browser/profile"),
            headless: true,
            viewport: EngineConfig::VIEWPORT,
            allow_no_sandbox: false,
        },
        search,
        Arc::new(move |l: &str| {
            sink.lock()
                .unwrap_or_else(|e| e.into_inner())
                .push(l.to_owned())
        }),
    )
    .no_sandbox(running_as_root() || eludite_browser::chrome::no_sandbox_from_env());
    let stats = engine.stats();
    let sink = log.clone();
    let mut run = Run {
        browser: Browser::new(
            Box::new(engine),
            Arc::new(move |l: &str| {
                sink.lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .push(l.to_owned())
            }),
        ),
        log: log.clone(),
    };
    let base = serve();

    // Before launch: not running.
    let tabs = run.ok(cmds::TABS, json!({}));
    assert_eq!(tabs["running"], false);
    assert_eq!(tabs["engine"]["name"], "embedded-chromium");

    // tab_open launches the engine and loads the form.
    let started = Instant::now();
    let form = format!("{base}/form.html");
    let opened = match run.call(cmds::TAB_OPEN, json!({"url": form})) {
        Ok(v) => v,
        Err(e) if e.to_string().contains("built without CEF") => {
            eludite_test_support::skip("cef", e);
            return;
        }
        Err(e) => panic!("tab_open: {e}\nlog: {:#?}", log.lock().unwrap()),
    };
    println!(
        "tab_open (engine cold start {:.0} ms + load form.html): {:.0} ms",
        stats.launch_us.load(Ordering::Relaxed) as f64 / 1e3,
        started.elapsed().as_secs_f64() * 1e3
    );
    assert_eq!(opened["launched"], true);
    assert_eq!(opened["id"], "t1");
    assert_eq!(opened["status"], 200, "{opened}");
    assert_eq!(opened["url"], form);
    assert!(
        log.lock()
            .unwrap()
            .iter()
            .any(|l| l.starts_with("Launched the embedded browser"))
    );
    let tabs = run.ok(cmds::TABS, json!({}));
    assert_eq!(tabs["running"], true);
    assert_eq!(tabs["tabs"].as_array().unwrap().len(), 1, "{tabs}");
    assert!(
        tabs["engine"]["version"]
            .as_str()
            .unwrap()
            .contains("Chromium/154.0.8037.58")
    );
    assert_eq!(tabs["tabs"][0]["title"], "Sign up");
    // Brief 0039: how the sandbox runs (engine/ready, shortly after the start) and how the engine and CEF were found.
    let mode = (0..100)
        .find_map(|_| {
            let t = run.ok(cmds::TABS, json!({}));
            let m = t["engine"]["sandbox"].as_str().map(str::to_owned);
            if m.is_none() {
                std::thread::sleep(Duration::from_millis(20));
            }
            m
        })
        .expect("engine.sandbox");
    if running_as_root() {
        assert_eq!(mode, "none", "{tabs}");
    } else {
        assert!(
            ["namespaces", "helper", "none"].contains(&mode.as_str()),
            "{mode}"
        );
    }
    assert!(tabs["engine"]["cef"].as_str().is_some(), "{tabs}");
    assert!(
        ["setting", "variable", "beside", "dev"].contains(
            &tabs["engine"]["found_by"]["engine"]
                .as_str()
                .unwrap_or_default()
        ),
        "{tabs}"
    );

    // navigate with load and network_idle.
    for page in ["list.html", "form.html"] {
        for wait in ["load", "network_idle"] {
            let url = format!("{base}/{page}");
            let nav = run.ok(
                cmds::NAVIGATE,
                json!({"url": url, "wait_until": wait, "wait_ms": 5000}),
            );
            assert_eq!(nav["status"], 200, "{page} {wait}: {nav}");
            assert_eq!(nav["timed_out"], false, "{nav}");
        }
    }

    // read_page: the form's controls with refs and boxes.
    let page = run.ok(cmds::READ_PAGE, json!({}));
    let nodes = page["nodes"].as_array().unwrap();
    let find_node = |role: &str, name: &str| {
        nodes
            .iter()
            .find(|n| n["role"] == role && n["name"] == name)
            .unwrap_or_else(|| panic!("no {role} {name} in {page}"))
            .clone()
    };
    let name_box = find_node("textbox", "Name");
    assert_eq!(name_box["value"], "Ada");
    assert!(name_box["ref"].as_str().unwrap().starts_with('e'));
    assert!(name_box["box"]["width"].as_f64().unwrap() > 10.);
    assert_eq!(find_node("checkbox", "Subscribe")["state"]["checked"], true);
    find_node("button", "Submit");
    find_node("link", "Terms");

    // evaluate.
    let v = run.ok(
        cmds::EVALUATE,
        json!({"expression": "document.title + ':' + (1 + 2)"}),
    );
    assert_eq!(v["result"], "Sign up:3");
    let v = run.ok(
        cmds::EVALUATE,
        json!({"expression": "new Promise(r => setTimeout(() => r({a: [1, 2]}), 50))"}),
    );
    assert_eq!(v["result"], json!({"a": [1, 2]}));
    assert!(
        run.call(
            cmds::EVALUATE,
            json!({"expression": "throw new Error('boom')"})
        )
        .is_err()
    );

    // screenshot from the ring: four colors where they belong.
    run.ok(cmds::NAVIGATE, json!({"url": QUADRANTS}));
    let ring_before = stats.ring_screenshots.load(Ordering::Relaxed);
    let shot = run.ok(cmds::SCREENSHOT, json!({}));
    assert_eq!(
        stats.ring_screenshots.load(Ordering::Relaxed),
        ring_before + 1,
        "from the ring"
    );
    assert_eq!(shot["format"], "png");
    let img = decode(shot["image"].as_str().unwrap());
    let (w, h) = img.dimensions();
    assert_eq!((w, h), (1280, 800), "{}", shot);
    assert_eq!(
        (shot["width"].as_u64(), shot["height"].as_u64()),
        (Some(1280), Some(800))
    );
    for (x, y, rgb) in [
        (w / 4, h / 4, [255, 0, 0]),
        (3 * w / 4, h / 4, [0, 128, 0]),
        (w / 4, 3 * h / 4, [0, 0, 255]),
        (3 * w / 4, 3 * h / 4, [0, 0, 0]),
    ] {
        let p = img.get_pixel(x, y).0;
        assert_eq!([p[0], p[1], p[2]], rgb, "at {x},{y}");
    }
    let small = run.ok(cmds::SCREENSHOT, json!({"max_width": 640}));
    assert_eq!(
        (small["width"].as_u64(), small["height"].as_u64()),
        (Some(640), Some(400))
    );
    let jpeg = run.ok(cmds::SCREENSHOT, json!({"format": "jpeg", "quality": 60}));
    assert!(jpeg["image"].as_str().unwrap().starts_with("/9j/"));
    // A full page is beyond the viewport: Page.captureScreenshot.
    let cdp_before = stats.cdp_screenshots.load(Ordering::Relaxed);
    let full = run.ok(cmds::SCREENSHOT, json!({"full_page": true}));
    assert!(full["height"].as_u64().unwrap() > 0);
    assert_eq!(
        stats.cdp_screenshots.load(Ordering::Relaxed),
        cdp_before + 1
    );
    let shot_ms: Vec<f64> = (0..20)
        .map(|_| {
            let t = Instant::now();
            run.ok(cmds::SCREENSHOT, json!({"max_width": 1280}));
            t.elapsed().as_secs_f64() * 1e3
        })
        .collect();
    let (p50, p95v, max) = p95(shot_ms);
    println!(
        "screenshot from the ring at 1280 wide (20): p50 {p50:.1} ms, p95 {p95v:.1} ms, max {max:.1} ms (budget p95 < 150 ms)"
    );

    // tab_close, then a second tab.
    let second = run.ok(cmds::TAB_OPEN, json!({"url": format!("{base}/list.html")}));
    assert_eq!(second["id"], "t2");
    assert_eq!(second["launched"], false);
    let closed = run.ok(cmds::TAB_CLOSE, json!({"tab": "t2"}));
    assert_eq!(closed["closed"], "t2");
    assert_eq!(
        run.ok(cmds::TABS, json!({}))["tabs"]
            .as_array()
            .unwrap()
            .len(),
        1
    );

    // The engine killed: the tabs are lost, the next command starts a new engine.
    let pid = stats.pid.load(Ordering::Relaxed);
    assert!(pid > 0);
    std::process::Command::new("kill")
        .args(["-9", &pid.to_string()])
        .status()
        .unwrap();
    let deadline = Instant::now() + eludite_test_support::hang_bound(Duration::from_secs(10));
    while !log
        .lock()
        .unwrap()
        .iter()
        .any(|l| l.contains("exited unexpectedly"))
    {
        assert!(
            Instant::now() < deadline,
            "the crash was not noticed: {:#?}",
            log.lock().unwrap()
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    let again = run.ok(cmds::TAB_OPEN, json!({"url": format!("{base}/form.html")}));
    assert_eq!(again["launched"], true, "{again}");
    assert_ne!(stats.pid.load(Ordering::Relaxed), pid);
    assert_eq!(
        run.ok(cmds::EVALUATE, json!({"expression": "document.title"}))["result"],
        "Sign up"
    );
    run.browser.shutdown();
}

/// The window's handle on the engine, once it started.
type Handle = Arc<Mutex<Option<eludite_browser::TabControl>>>;

/// The engine with a [`Browser`] over it and the window's handle (brief 0032), or `None` when it cannot run here.
fn window_run() -> Option<(Run, Handle, tempfile::TempDir)> {
    if !cfg!(target_os = "linux") {
        println!("SKIPPED: the embedded engine runs on Linux only so far (brief 0031)");
        return None;
    }
    let search = ChromiumSearch::defaults();
    if let Err(why) = search.find_engine().and_then(|e| search.find_cef(&e)) {
        eludite_test_support::skip("cef", why);
        return None;
    }
    let profile = tempfile::tempdir().unwrap();
    let log: Arc<Mutex<Vec<String>>> = Arc::default();
    let sink = log.clone();
    let sink2 = log.clone();
    let control: Handle = Arc::default();
    let c = control.clone();
    let mut engine = EmbeddedChromium::new(
        EngineConfig {
            executable: None,
            profile_dir: profile.path().join(".eludite/browser/profile"),
            headless: true,
            viewport: (800, 600),
            allow_no_sandbox: false,
        },
        search,
        Arc::new(move |l: &str| {
            sink.lock()
                .unwrap_or_else(|e| e.into_inner())
                .push(l.to_owned())
        }),
    )
    .no_sandbox(running_as_root() || eludite_browser::chrome::no_sandbox_from_env());
    engine.set_observer(Some(Arc::new(move |e| {
        if let eludite_browser::EngineEvent::Started(t) = e {
            *c.lock().unwrap() = Some(t);
        }
    })));
    let run = Run {
        browser: Browser::new(
            Box::new(engine),
            Arc::new(move |l: &str| {
                sink2
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .push(l.to_owned())
            }),
        ),
        log,
    };
    Some((run, control, profile))
}

fn data_url(html: &str) -> String {
    let mut s = String::from("data:text/html,");
    for b in html.bytes() {
        if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) {
            s.push(b as char);
        } else {
            s.push_str(&format!("%{b:02X}"));
        }
    }
    s
}

#[test]
fn the_window_keys_dialogs_devtools_record_and_click_to_frame() {
    let Some((mut run, control, profile)) = window_run() else {
        return;
    };
    // Every key the window sends reaches the page with its Windows key code, DOM key and DOM code.
    let page = "<html><head><title>keys</title></head><body><input id=i autofocus><script>\
        window.seen = []; addEventListener('keydown', e => { seen.push([e.keyCode, e.key, e.code]); \
        if (e.key === 'Tab' || e.key === 'F5' || e.key === 'Backspace' || e.key === ' ') e.preventDefault(); }, true);\
        </script></body></html>";
    let opened = run.ok(cmds::TAB_OPEN, json!({"url": data_url(page)}));
    assert_eq!(opened["title"], "keys");
    let tabs = run.ok(cmds::TABS, json!({}));
    assert_eq!(tabs["engine"]["name"], "embedded-chromium");
    // tab_open loads about:blank, then the url: there is a page to go back to.
    assert_eq!(tabs["tabs"][0]["can_go_back"], true);
    assert_eq!(tabs["tabs"][0]["can_go_forward"], false);
    let target = run.browser.targets_of_tabs()[0].1.clone();
    let ctl = control
        .lock()
        .unwrap()
        .clone()
        .expect("Started handed the window its handle");
    for key in eludite_browser::keys::SHELL_KEYS {
        for t in ["rawKeyDown", "keyUp"] {
            ctl.input(
                &target,
                json!({"type": t, "windowsKeyCode": key.windows, "nativeKeyCode": key.x11}),
            );
        }
    }
    let n = eludite_browser::keys::SHELL_KEYS.len();
    let deadline = Instant::now() + eludite_test_support::hang_bound(Duration::from_secs(20));
    let seen = loop {
        let v = run.ok(cmds::EVALUATE, json!({"expression": "window.seen"}))["result"].clone();
        if v.as_array().is_some_and(|a| a.len() >= n) || Instant::now() > deadline {
            break v;
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    let seen = seen.as_array().unwrap();
    assert_eq!(seen.len(), n, "every key arrived: {seen:?}");
    for (key, got) in eludite_browser::keys::SHELL_KEYS.iter().zip(seen) {
        assert_eq!(got[0], key.windows, "{} keyCode: {got}", key.gpui);
        assert_eq!(got[2], key.dom_code, "{} code: {got}", key.gpui);
        assert_eq!(
            got[1].as_str().unwrap().to_lowercase(),
            key.dom_key.to_lowercase(),
            "{} key: {got}",
            key.gpui
        );
    }

    // An agent's click that opens a dialog gets it in its answer; dialog answers it.
    let page = "<html><head><title>ask</title></head><body style='margin:0'>\
        <button id=b style='width:300px;height:100px' onclick=\"document.title = confirm('Delete it?') ? 'yes' : 'no'\">x</button>\
        </body></html>";
    run.ok(cmds::NAVIGATE, json!({"url": data_url(page)}));
    let t0 = Instant::now();
    let out = run.ok(
        cmds::INPUT,
        json!({"action": "click", "x": 50, "y": 50, "wait_ms": 2000}),
    );
    eludite_test_support::assert_budget(
        "a click that opens a confirm dialog",
        t0.elapsed(),
        Duration::from_secs(2),
    );
    assert_eq!(
        out["dialog"],
        json!({"kind": "confirm", "message": "Delete it?"}),
        "{out}"
    );
    let a = run.ok(cmds::DIALOG, json!({"action": "accept"}));
    assert_eq!(a["kind"], "confirm");
    let w = run.ok(
        cmds::WAIT,
        json!({"for": "function", "expression": "document.title === 'yes'", "wait_ms": 5000}),
    );
    assert_eq!(w["timeout"], false, "{w}");

    // DevTools opens as a tab of its own, which `tabs` does not list (it is not a page).
    let d = run.ok(cmds::DEVTOOLS, json!({}));
    assert_eq!(d["opened"], true);
    let tabs = run.ok(cmds::TABS, json!({}));
    assert_eq!(tabs["tabs"].as_array().unwrap().len(), 1, "{tabs}");
    assert!(
        ctl.tabs().len() >= 2,
        "the window sees DevTools: {:?}",
        ctl.tabs()
    );
    assert_eq!(run.ok(cmds::DEVTOOLS, json!({}))["opened"], false);

    // record: the tab's frames into a GIF while a page animates.
    let anim = "<html><head><title>anim</title></head><body style='margin:0'><div id=d style='width:100px;height:100px;background:red'></div>\
        <script>let n=0; setInterval(() => { n++; d.style.background = n % 2 ? 'blue' : 'red'; }, 50);</script></body></html>";
    run.ok(cmds::NAVIGATE, json!({"url": data_url(anim)}));
    // The page animates before the recording starts (a loaded machine paints late).
    let ring = ctl.frames(&target).unwrap();
    let seq0 = eludite_browser::FrameSource::sequence(ring.as_ref());
    let deadline = Instant::now() + eludite_test_support::hang_bound(Duration::from_secs(10));
    while eludite_browser::FrameSource::sequence(ring.as_ref()) < seq0 + 2 {
        assert!(Instant::now() < deadline, "the animation never painted");
        std::thread::sleep(Duration::from_millis(10));
    }
    let gif = profile.path().join("anim.gif");
    run.ok(
        cmds::RECORD,
        json!({"action": "start", "path": gif, "fps": 10}),
    );
    std::thread::sleep(Duration::from_millis(1500));
    let done = run.ok(cmds::RECORD, json!({"action": "stop"}));
    let frames = done["frames"].as_u64().unwrap();
    let low = if busy() { 2 } else { 8 };
    assert!(
        (low..=16).contains(&frames),
        "about 15 ticks, nearly every one a change: {done}"
    );
    assert_eq!(
        (done["width"].as_u64(), done["height"].as_u64()),
        (Some(800), Some(600))
    );
    assert!(std::fs::read(&gif).unwrap().starts_with(b"GIF89a"));
    eprintln!(
        "record: {frames} frames, {} bytes in {}",
        done["bytes"],
        gif.display()
    );

    // `input` click to the next painted frame (the ring's sequence): the page turns the box green on mousedown.
    let page = "<html><head><title>click</title></head><body style='margin:0'><div id=d style='width:200px;height:200px;background:#000'></div>\
        <script>let on = false; d.addEventListener('mousedown', () => { on = !on; d.style.background = on ? '#0f0' : '#000'; });</script></body></html>";
    run.ok(cmds::NAVIGATE, json!({"url": data_url(page)}));
    let frames = ctl.frames(&target).unwrap();
    let mut samples = Vec::new();
    for i in 0..20 {
        let want: [u8; 4] = if i % 2 == 0 {
            [0, 255, 0, 255]
        } else {
            [0, 0, 0, 255]
        };
        let seq0 = eludite_browser::FrameSource::sequence(frames.as_ref());
        let t0 = Instant::now();
        run.ok(
            cmds::INPUT,
            json!({"action": "click", "x": 100, "y": 100, "wait_ms": 0}),
        );
        loop {
            let mut px = [0u8; 4];
            let seq = eludite_browser::FrameSource::sequence(frames.as_ref());
            if seq > seq0 {
                eludite_browser::FrameSource::read(frames.as_ref(), false, &mut |f| {
                    let o = (100 * f.stride + 100 * 4) as usize;
                    px.copy_from_slice(&f.pixels[o..o + 4]);
                });
                if [px[2], px[1], px[0], px[3]] == want {
                    break;
                }
            }
            assert!(
                t0.elapsed() < eludite_test_support::hang_bound(Duration::from_secs(5)),
                "no frame with the click's change"
            );
            std::thread::sleep(Duration::from_micros(500));
        }
        samples.push(t0.elapsed().as_secs_f64() * 1e3);
    }
    let (p50, p95, max) = p95(samples);
    eprintln!(
        "input click to the next frame in the ring: p50 {p50:.1} ms, p95 {p95:.1} ms, max {max:.1} ms"
    );
    if !busy() {
        assert!(
            p95 < 50.,
            "input click to frame p95 {p95:.1} ms (budget 50 ms)"
        );
    }
    run.browser.shutdown();
}

/// A CI run or a loaded machine: timing budgets and rates are reported, not asserted
/// (`eludite_test_support::budgets_enforced`).
fn busy() -> bool {
    !eludite_test_support::budgets_enforced()
}
