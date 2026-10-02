//! Brief 0023's browser commands against the embedded engine (brief 0031): `eludite-chromium` (CEF, out of process,
//! frames in shared memory) as the [`Engine`] of an unchanged [`Browser`]. The subset the brief names: `tab_open`,
//! `navigate`, `screenshot` (from the frame ring, checked pixel by pixel; `Page.captureScreenshot` for a full page),
//! `read_page` and `evaluate`, then `tab_close`, and the engine killed and relaunched by the next command.
//!
//! Skips with a message when the engine is not built with CEF (`tools/cef/fetch.sh`, then `CEF_PATH=...
//! cargo build -p eludite-chromium --features eludite-chromium/cef`) or off Linux. As root the engine gets
//! `ELUDITE_CHROME_NO_SANDBOX=1` (as brief 0023's Chrome tests do). Timings print with `-- --nocapture`.

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
        println!("SKIPPED: {why}");
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
            println!("SKIPPED: {e}");
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
    let deadline = Instant::now() + Duration::from_secs(10);
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
