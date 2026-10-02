//! The browser commands against a real, headless Chrome or Chromium (brief 0023). Skips with a message when none
//! is found (ELUDITE_CHROME, `tools/chrome/fetch.sh`'s cache, PATH, the usual install locations).
//!
//! A std-only HTTP server in this test serves `tests/fixtures/`. One Chrome is launched for the whole run (a launch
//! costs about a second), and the steps run in order: tabs before launch, tab_open, navigate with `load` and
//! `network_idle`, screenshot, read_page, find, page_text, console, network, wait, evaluate, a stale ref after the
//! document is rewritten, tab_close, then Chrome killed under a pending request and relaunched. The budgets' timings
//! are printed (`cargo test -p eludite-browser --test chrome -- --nocapture`).
//!
//! `ELUDITE_RECORD_FIXTURES=1` also rewrites `tests/fixtures/form-axtree.json` (the recorded accessibility tree the
//! unit tests filter) from this Chrome.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use eludite_browser::{Browser, ChromeSearch, Engine, EngineConfig, ExternalChrome};
use eludite_commands::CommandError;
use eludite_commands::browser::{self as cmds, BrowserOutput, BrowserRequest};
use serde_json::{Value, json};

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

/// Serve `tests/fixtures/` on 127.0.0.1; anything else is a 404. One thread per connection, `Connection: close`.
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
    let ok = !path.is_empty() && !path.contains("..") && file.is_file();
    let (status, body, mime) = if ok {
        let mime = if path.ends_with(".html") {
            "text/html; charset=utf-8"
        } else {
            "application/json"
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

/// The Chrome to test: on Linux whatever discovery finds, elsewhere only `ELUDITE_CHROME` (CI runs these tests on
/// Linux only, and the Windows and macOS runners have a Chrome installed).
fn chrome() -> Option<PathBuf> {
    if !cfg!(target_os = "linux") && std::env::var_os("ELUDITE_CHROME").is_none_or(|v| v.is_empty())
    {
        println!("SKIPPED: the browser tests run on Linux, or where ELUDITE_CHROME names a Chrome");
        return None;
    }
    match ChromeSearch::from_env().find() {
        Ok(p) => Some(p),
        Err(why) => {
            println!("SKIPPED: no Chrome for the browser tests ({why})");
            None
        }
    }
}

struct Run {
    browser: Browser,
    base: String,
    log: Arc<Mutex<Vec<String>>>,
}

impl Run {
    fn new(exe: &Path, profile: &Path) -> (Run, Arc<Mutex<Vec<String>>>) {
        let log: Arc<Mutex<Vec<String>>> = Arc::default();
        let sink = log.clone();
        let engine = ExternalChrome::new(
            EngineConfig {
                executable: Some(exe.to_path_buf()),
                profile_dir: profile.to_path_buf(),
                headless: true,
                viewport: EngineConfig::VIEWPORT,
            },
            ChromeSearch::default(),
            Arc::new(move |l: &str| sink.lock().unwrap().push(l.to_owned())),
        )
        .no_sandbox(running_as_root() || eludite_browser::chrome::no_sandbox_from_env());
        let sink = log.clone();
        let browser = Browser::new(
            Box::new(engine),
            Arc::new(move |l: &str| sink.lock().unwrap().push(l.to_owned())),
        );
        (
            Run {
                browser,
                base: serve(),
                log: log.clone(),
            },
            log,
        )
    }

    fn call(&mut self, id: &str, input: Value) -> Result<Value, CommandError> {
        let request = cmds::parse(id, input)?;
        self.browser.apply(request).map(|o| o.to_json())
    }

    fn ok(&mut self, id: &str, input: Value) -> Value {
        match self.call(id, input.clone()) {
            Ok(v) => v,
            Err(e) => panic!("{id} {input}: {e}"),
        }
    }

    fn url(&self, page: &str) -> String {
        format!("{}/{page}", self.base)
    }
}

fn p95(mut v: Vec<f64>) -> (f64, f64, f64) {
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let at = |q: f64| v[((v.len() as f64 * q).ceil() as usize).clamp(1, v.len()) - 1];
    (at(0.5), at(0.95), *v.last().unwrap())
}

fn timed(mut f: impl FnMut()) -> f64 {
    let t = Instant::now();
    f();
    t.elapsed().as_secs_f64() * 1e3
}

#[test]
fn the_commands_against_a_headless_chrome() {
    let Some(exe) = chrome() else { return };
    let profile = tempfile::tempdir().unwrap();
    let (mut run, log) = Run::new(&exe, &profile.path().join(".eludite/browser/profile"));
    println!("chrome: {}", exe.display());

    // Before launch: not running, nothing launched.
    let tabs = run.ok(cmds::TABS, json!({}));
    assert_eq!(tabs["running"], false);
    assert_eq!(tabs["engine"]["name"], "external-chrome");
    assert!(log.lock().unwrap().is_empty(), "tabs launched nothing");
    assert!(
        run.call(cmds::READ_PAGE, json!({}))
            .unwrap_err()
            .to_string()
            .contains("no tab")
    );

    // tab_open with a url launches, opens, selects, loads.
    let rss_before = resident_kb();
    let started = Instant::now();
    let form_url = run.url("form.html");
    let opened = run.ok(cmds::TAB_OPEN, json!({"url": form_url}));
    let launch_ms = started.elapsed().as_secs_f64() * 1e3;
    println!("tab_open (launch + load form.html): {launch_ms:.0} ms");
    assert_eq!(opened["launched"], true);
    assert_eq!(opened["id"], "t1");
    assert_eq!(opened["active"], true);
    assert_eq!(opened["status"], 200);
    assert_eq!(opened["url"], form_url);
    assert!(
        log.lock()
            .unwrap()
            .iter()
            .any(|l| l.starts_with("Launched ")),
        "{:?}",
        log.lock().unwrap()
    );
    let tabs = run.ok(cmds::TABS, json!({}));
    assert_eq!(tabs["running"], true);
    assert_eq!(
        tabs["tabs"].as_array().unwrap().len(),
        1,
        "the initial blank tab was reused: {tabs}"
    );
    let version = tabs["engine"]["version"].as_str().unwrap().to_owned();
    println!("engine: {version}");
    assert!(version.contains("Chrome"));
    assert_eq!(tabs["tabs"][0]["title"], "Sign up");
    // This process's memory with the browser running and idle (Chrome's own processes are separate).
    std::thread::sleep(Duration::from_millis(500));
    if let (Some(before), Some(after)) = (rss_before, resident_kb()) {
        let grew = after.saturating_sub(before) as f64 / 1024.;
        println!(
            "resident memory with the browser running and idle: +{grew:.1} MB (budget +20 MB)"
        );
        assert!(grew < 20., "+{grew:.1} MB");
    }

    // navigate to each fixture with load and network_idle.
    for page in ["list.html", "errors.html", "rewrite.html", "form.html"] {
        for wait in ["load", "network_idle"] {
            let url = run.url(page);
            let nav = run.ok(
                cmds::NAVIGATE,
                json!({"url": url, "wait_until": wait, "wait_ms": 5000}),
            );
            if nav["timed_out"] == true {
                let net = run.ok(cmds::NETWORK, json!({}));
                panic!("{page} {wait}: {nav}\n{net:#}");
            }
            assert_eq!(nav["status"], 200, "{page} {wait}: {nav}");
            assert_eq!(nav["url"], url);
            if page == "errors.html" {
                assert!(nav["console_errors"].as_u64().unwrap() >= 1, "{nav}");
            }
            if wait == "network_idle" {
                assert!(nav["elapsed_ms"].as_f64().unwrap() >= 500., "{nav}");
            }
        }
    }
    let gen_before = run.ok(cmds::TABS, json!({}))["tabs"][0]["page_generation"]
        .as_u64()
        .unwrap();
    let nav = run.ok(cmds::NAVIGATE, json!({"action": "back"}));
    assert!(
        nav["url"].as_str().unwrap().ends_with("rewrite.html"),
        "{nav}"
    );
    assert!(nav["page_generation"].as_u64().unwrap() > gen_before);
    let nav = run.ok(cmds::NAVIGATE, json!({"action": "forward"}));
    assert!(nav["url"].as_str().unwrap().ends_with("form.html"), "{nav}");
    let nav = run.ok(
        cmds::NAVIGATE,
        json!({"action": "reload", "wait_until": "domcontentloaded"}),
    );
    assert_eq!(nav["timed_out"], false);
    // A navigation error fails the command and names it.
    let err = run
        .call(
            cmds::NAVIGATE,
            json!({"url": "http://127.0.0.1:9/nothing-listens-here"}),
        )
        .unwrap_err()
        .to_string();
    assert!(err.contains("net::ERR_"), "{err}");
    assert!(
        log.lock()
            .unwrap()
            .iter()
            .any(|l| l.contains("Navigation to") && l.contains("net::ERR_"))
    );
    let nav_ms: Vec<f64> = (0..20)
        .map(|_| {
            let url = run.url("form.html");
            timed(|| {
                run.ok(cmds::NAVIGATE, json!({"url": url}));
            })
        })
        .collect();
    let (p50, p95v, max) = p95(nav_ms);
    println!(
        "navigate to form.html with load (20): p50 {p50:.1} ms, p95 {p95v:.1} ms, max {max:.1} ms (budget p95 < 500 ms)"
    );

    // screenshot: PNG of the viewport, scaled to max_width.
    let shot = run.ok(cmds::SCREENSHOT, json!({}));
    assert_eq!(shot["format"], "png");
    let image = shot["image"].as_str().unwrap();
    assert!(image.starts_with("iVBORw0KGgo"), "a PNG");
    let viewport = run.ok(
        cmds::EVALUATE,
        json!({"expression": "[innerWidth, innerHeight, devicePixelRatio]"}),
    )["result"]
        .clone();
    let (vw, vh, dpr) = (
        viewport[0].as_f64().unwrap(),
        viewport[1].as_f64().unwrap(),
        viewport[2].as_f64().unwrap(),
    );
    println!(
        "viewport {vw}x{vh} at {dpr}; screenshot {}x{}",
        shot["width"], shot["height"]
    );
    assert_eq!(
        shot["width"].as_f64().unwrap(),
        (vw * dpr).min(1280.).round()
    );
    let small = run.ok(cmds::SCREENSHOT, json!({"max_width": 640}));
    assert_eq!(small["width"], 640);
    let expect_h = (vh * 640. / vw).round();
    assert!(
        (small["height"].as_f64().unwrap() - expect_h).abs() <= 1.,
        "{} vs {expect_h}",
        small["height"]
    );
    assert!((small["scale"].as_f64().unwrap() - 640. / vw).abs() < 0.01);
    let jpeg = run.ok(cmds::SCREENSHOT, json!({"format": "jpeg", "quality": 60}));
    assert!(jpeg["image"].as_str().unwrap().starts_with("/9j/"));
    assert!(jpeg["width"].as_u64().unwrap() > 0);
    let full = run.ok(cmds::SCREENSHOT, json!({"full_page": true}));
    assert!(full["height"].as_u64().unwrap() > 0);
    let shot_ms: Vec<f64> = (0..20)
        .map(|_| {
            timed(|| {
                run.ok(cmds::SCREENSHOT, json!({"max_width": 1280}));
            })
        })
        .collect();
    let (p50, p95v, max) = p95(shot_ms);
    println!(
        "screenshot at 1280 wide (20): p50 {p50:.1} ms, p95 {p95v:.1} ms, max {max:.1} ms (budget p95 < 150 ms)"
    );

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
    assert_eq!(find_node("textbox", "Email")["state"]["required"], true);
    assert_eq!(find_node("checkbox", "Subscribe")["state"]["checked"], true);
    find_node("combobox", "Plan");
    find_node("button", "Submit");
    assert_eq!(find_node("button", "Disabled")["state"]["disabled"], true);
    find_node("link", "Terms");
    assert_eq!(page["truncated"], false);
    let dom = run.ok(cmds::READ_PAGE, json!({"mode": "dom"}));
    assert!(
        dom["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .any(|n| n["role"] == "button" && n["name"] == "Submit")
    );

    if std::env::var_os("ELUDITE_RECORD_FIXTURES").is_some() {
        record_ax_tree();
    }

    // find by css, text, role and name.
    let by_css = run.ok(cmds::FIND, json!({"css": "input"}));
    assert_eq!(by_css["total"], 3, "{by_css}");
    assert_eq!(by_css["matches"][0]["role"], "textbox");
    assert_eq!(by_css["matches"][0]["name"], "Name");
    assert!(by_css["matches"][0]["box"]["height"].as_f64().unwrap() > 0.);
    let by_text = run.ok(cmds::FIND, json!({"text": "Terms"}));
    assert!(
        by_text["matches"]
            .as_array()
            .unwrap()
            .iter()
            .any(|m| m["role"] == "link" && m["name"] == "Terms"),
        "{by_text}"
    );
    let by_role = run.ok(cmds::FIND, json!({"role": "button", "name": "subm"}));
    assert_eq!(by_role["total"], 1);
    assert_eq!(by_role["matches"][0]["name"], "Submit");
    // The same element keeps its ref within a generation.
    assert_eq!(
        by_role["matches"][0]["ref"],
        find_node("button", "Submit")["ref"]
    );
    let none = run.ok(cmds::FIND, json!({"css": "table"}));
    assert_eq!(none["total"], 0);
    assert!(run.call(cmds::FIND, json!({"css": "[["})).is_err());

    // screenshot of a ref.
    let clip = run.ok(
        cmds::SCREENSHOT,
        json!({"clip": by_role["matches"][0]["ref"]}),
    );
    assert!(clip["width"].as_u64().unwrap() < 200, "{}", clip["width"]);

    // wait: a selector the page adds after a timeout, and a function.
    run.ok(cmds::NAVIGATE, json!({"url": run.url("form.html")}));
    let w = run.ok(
        cmds::WAIT,
        json!({"for": "selector", "css": "#late", "wait_ms": 5000}),
    );
    assert_eq!(w["timeout"], false, "{w}");
    assert!(w["satisfied"]["ref"].as_str().unwrap().starts_with('e'));
    println!(
        "wait for #late (set at 400 ms after load): {} ms",
        w["elapsed_ms"]
    );
    let w = run.ok(
        cmds::WAIT,
        json!({"for": "function", "expression": "window.lateReady === true"}),
    );
    assert_eq!(w["satisfied"]["value"], true);
    let w = run.ok(cmds::WAIT, json!({"for": "text", "text": "Late paragraph"}));
    assert_eq!(w["timeout"], false);
    let w = run.ok(
        cmds::WAIT,
        json!({"for": "selector", "css": "#never", "wait_ms": 300}),
    );
    assert_eq!(w["timeout"], true);
    assert!(w.get("satisfied").is_none());
    let w = run.ok(cmds::WAIT, json!({"for": "network_idle"}));
    assert_eq!(w["timeout"], false);

    // page_text, paged.
    let whole = run.ok(cmds::PAGE_TEXT, json!({}));
    let whole_text = whole["text"].as_str().unwrap().to_owned();
    assert!(whole_text.starts_with("Sign up"), "{whole_text:?}");
    let t = run.ok(cmds::PAGE_TEXT, json!({"max_chars": 10}));
    assert_eq!(
        t["text"].as_str().unwrap(),
        whole_text.chars().take(10).collect::<String>()
    );
    assert_eq!(t["next"], 10);
    let total = t["total_chars"].as_u64().unwrap();
    assert_eq!(total, whole_text.chars().count() as u64);
    let rest = run.ok(cmds::PAGE_TEXT, json!({"cursor": 10, "max_chars": 100000}));
    assert!(rest.get("next").is_none());
    assert_eq!(
        rest["text"].as_str().unwrap().chars().count() as u64,
        total - 10
    );
    assert!(rest["text"].as_str().unwrap().contains("Late paragraph"));
    let form_ref = find_ref(&mut run, "form", "Sign up");
    let in_form = run.ok(cmds::PAGE_TEXT, json!({"root": form_ref}));
    assert!(!in_form["text"].as_str().unwrap().contains("Terms"));

    // console and network on the errors page.
    let since_console = run.ok(cmds::CONSOLE, json!({}))["next"].as_u64().unwrap();
    let since_network = run.ok(cmds::NETWORK, json!({}))["next"].as_u64().unwrap();
    let errors_url = run.url("errors.html");
    run.ok(
        cmds::NAVIGATE,
        json!({"url": errors_url, "wait_until": "network_idle"}),
    );
    let c = run.ok(
        cmds::CONSOLE,
        json!({"since": since_console, "level": "error"}),
    );
    let boom = c["messages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["text"] == "boom from errors.html")
        .unwrap_or_else(|| panic!("{c}"))
        .clone();
    assert_eq!(boom["url"], errors_url);
    assert_eq!(
        boom["line"], 11,
        "console.error is on line 11 of errors.html: {boom}"
    );
    assert!(boom["stack"].as_str().unwrap().contains("errors.html:11"));
    let all = run.ok(cmds::CONSOLE, json!({"since": since_console}));
    assert!(
        all["messages"]
            .as_array()
            .unwrap()
            .iter()
            .any(|m| m["level"] == "log" && m["text"] == "errors.html loaded")
    );
    let warn = run.ok(
        cmds::CONSOLE,
        json!({"since": since_console, "pattern": "/missing\\.json answered 40\\d/"}),
    );
    assert_eq!(warn["messages"].as_array().unwrap().len(), 1, "{warn}");
    assert!(
        log.lock()
            .unwrap()
            .iter()
            .any(|l| l.contains("errors.html:11: boom from errors.html")),
        "console errors reach the log as [tab] url:line: text: {:?}",
        log.lock().unwrap()
    );
    let n = run.ok(
        cmds::NETWORK,
        json!({"since": since_network, "status": 404}),
    );
    let missing = &n["requests"][0];
    assert!(
        missing["url"].as_str().unwrap().ends_with("/missing.json"),
        "{n}"
    );
    assert_eq!(missing["status"], 404);
    assert_eq!(missing["method"], "GET");
    assert_eq!(missing["resource_type"], "Fetch");
    assert_eq!(missing["initiator"]["type"], "script");
    let docs = run.ok(
        cmds::NETWORK,
        json!({"since": since_network, "resource_type": "document"}),
    );
    assert_eq!(docs["requests"][0]["status"], 200);
    assert!(
        docs["requests"][0]["duration_ms"].as_f64().is_some(),
        "{docs}"
    );
    assert!(
        docs["requests"][0]["encoded_bytes"].as_u64().unwrap() > 0,
        "{docs}"
    );
    let filtered = run.ok(
        cmds::NETWORK,
        json!({"since": since_network, "url_pattern": "missing"}),
    );
    assert_eq!(filtered["requests"].as_array().unwrap().len(), 1);

    // evaluate: an object, a string cut at max_chars, an exception as a failed command.
    let e = run.ok(
        cmds::EVALUATE,
        json!({"expression": "({a: 1, b: [true, 'x'], c: null})"}),
    );
    assert_eq!(e["result"], json!({"a": 1, "b": [true, "x"], "c": null}));
    assert_eq!(e["type"], "object");
    assert_eq!(e["truncated"], false);
    let e = run.ok(
        cmds::EVALUATE,
        json!({"expression": "'x'.repeat(50)", "max_chars": 20}),
    );
    assert_eq!(e["result"], "x".repeat(20));
    assert_eq!(e["truncated"], true);
    let e = run.ok(
        cmds::EVALUATE,
        json!({"expression": "new Promise(r => setTimeout(() => r(42), 50))"}),
    );
    assert_eq!(e["result"], 42);
    let e = run.ok(
        cmds::EVALUATE,
        json!({"expression": "document.body", "return_by_value": false}),
    );
    assert_eq!(e["subtype"], "node");
    assert_eq!(e["result"], "body");
    let err = run
        .call(cmds::EVALUATE, json!({"expression": "null.boom"}))
        .unwrap_err();
    assert!(matches!(err, CommandError::Failed(_)));
    assert!(err.to_string().contains("TypeError"), "{err}");

    // The rewrite button bumps page_generation and a stale ref is refused.
    run.ok(cmds::NAVIGATE, json!({"url": run.url("rewrite.html")}));
    let before = run.ok(cmds::FIND, json!({"role": "button", "name": "Rewrite"}));
    let stale = before["matches"][0]["ref"].as_str().unwrap().to_owned();
    let gen0 = before["page_generation"].as_u64().unwrap();
    run.ok(
        cmds::EVALUATE,
        json!({"expression": "document.getElementById('rewrite').click()"}),
    );
    let w = run.ok(cmds::WAIT, json!({"for": "text", "text": "After"}));
    assert_eq!(w["timeout"], false);
    let gen1 = w["page_generation"].as_u64().unwrap();
    assert!(
        gen1 > gen0,
        "rewriting the document changes the generation ({gen0} -> {gen1})"
    );
    let err = run
        .call(cmds::SCREENSHOT, json!({"clip": stale}))
        .unwrap_err()
        .to_string();
    assert!(
        err.contains(&format!(
            "stale ref: the page changed (generation {gen1}, ref from {gen0}); call read_page again"
        )),
        "{err}"
    );
    let after = run.ok(cmds::READ_PAGE, json!({}));
    assert!(
        after["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .any(|n| n["name"] == "Again")
    );

    // read_page on 5,000 items: the budget, and the answer's size.
    run.ok(cmds::NAVIGATE, json!({"url": run.url("list.html")}));
    let big = run.ok(cmds::READ_PAGE, json!({}));
    assert_eq!(big["total"], 5000, "every link is interactive");
    assert_eq!(big["nodes"].as_array().unwrap().len(), 500);
    assert_eq!(big["truncated"], true);
    let size = serde_json::to_string(&big).unwrap().len();
    println!(
        "read_page on 5,000 items: {} nodes listed of {}, answer {size} bytes (budget < 64 KB)",
        500, big["total"]
    );
    assert!(size < 64 * 1024, "{size} bytes");
    let read_ms: Vec<f64> = (0..20)
        .map(|_| {
            timed(|| {
                run.ok(cmds::READ_PAGE, json!({}));
            })
        })
        .collect();
    let (p50, p95v, max) = p95(read_ms);
    println!(
        "read_page on the 5,000-item page (20): p50 {p50:.1} ms, p95 {p95v:.1} ms, max {max:.1} ms (budget p95 < 300 ms)"
    );

    // resize: the viewport in effect, as the page reports it.
    let r = run.ok(cmds::RESIZE, json!({"width": 390, "height": 844, "device_scale_factor": 2, "user_agent": "EluditeTest/1.0"}));
    assert_eq!(
        (r["width"].as_u64(), r["height"].as_u64()),
        (Some(390), Some(844)),
        "{r}"
    );
    assert_eq!(r["device_scale_factor"], 2.0);
    assert_eq!(r["user_agent"], "EluditeTest/1.0");
    assert_eq!(
        run.ok(cmds::EVALUATE, json!({"expression": "navigator.userAgent"}))["result"],
        "EluditeTest/1.0"
    );
    // The screenshot is the visual viewport, without the list page's vertical scrollbar (15 CSS pixels here).
    let client = run.ok(
        cmds::EVALUATE,
        json!({"expression": "document.documentElement.clientWidth"}),
    )["result"]
        .as_u64()
        .unwrap();
    assert!((370..=390).contains(&client), "{client}");
    let shot = run.ok(cmds::SCREENSHOT, json!({}));
    assert_eq!(shot["width"].as_u64(), Some(client * 2), "CSS pixels at 2x");
    // Mobile emulation of a page without a meta viewport lays it out 980 CSS pixels wide, as a phone would.
    let r = run.ok(
        cmds::RESIZE,
        json!({"width": 390, "height": 844, "mobile": true}),
    );
    assert_eq!(r["width"], 980, "{r}");
    assert_eq!(r["mobile"], true);

    // A second tab, select, close.
    let second = run.ok(cmds::TAB_OPEN, json!({}));
    assert_eq!(second["id"], "t2");
    assert_eq!(second["launched"], false);
    assert_eq!(second["url"], "about:blank");
    let sel = run.ok(cmds::TAB_SELECT, json!({"tab": "t1"}));
    assert_eq!(sel["active"], true);
    let closed = run.ok(cmds::TAB_CLOSE, json!({"tab": "t2"}));
    assert_eq!(
        (
            closed["closed"].as_str(),
            closed["tabs"].as_u64(),
            closed["active"].as_str()
        ),
        (Some("t2"), Some(1), Some("t1"))
    );
    assert!(run.call(cmds::TAB_SELECT, json!({"tab": "t2"})).is_err());
    assert!(log.lock().unwrap().iter().any(|l| l == "Closed tab t2"));

    // Kill Chrome under a pending request: the request fails, the engine is gone, the next tab_open relaunches.
    // (Unix only: the browser's pid comes from `ps`.)
    if cfg!(unix) {
        kill_and_relaunch(&mut run, profile.path());
    }

    run.browser.shutdown();
    assert!(
        run.log
            .lock()
            .unwrap()
            .iter()
            .any(|l| l == "Closed the browser.")
    );
    assert!(profile.path().join(".eludite/browser/.gitignore").is_file());
}

/// Kill Chrome with `kill -9` under a pending request, then open a tab again.
fn kill_and_relaunch(run: &mut Run, profile: &Path) {
    let pid = chrome_pid(&profile.join(".eludite/browser/profile"));
    let killer = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(300));
        let _ = std::process::Command::new("kill")
            .args(["-9", &pid.to_string()])
            .status();
    });
    let pending = run.call(
        cmds::WAIT,
        json!({"for": "function", "expression": "new Promise(() => {})", "wait_ms": 20000}),
    );
    killer.join().unwrap();
    let err = pending.unwrap_err().to_string();
    assert!(
        err.contains("not running") || err.contains("closed"),
        "{err}"
    );
    let tabs = run.ok(cmds::TABS, json!({}));
    assert_eq!(tabs["running"], false, "{tabs}");
    let until = Instant::now() + Duration::from_secs(5);
    while !run
        .log
        .lock()
        .unwrap()
        .iter()
        .any(|l| l.contains("exited unexpectedly"))
        && Instant::now() < until
    {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(
        run.log
            .lock()
            .unwrap()
            .iter()
            .any(|l| l.contains("exited unexpectedly")),
        "{:?}",
        run.log.lock().unwrap()
    );
    let again = run.ok(cmds::TAB_OPEN, json!({"url": run.url("form.html")}));
    assert_eq!(again["launched"], true);
    assert_eq!(again["status"], 200);
    assert_eq!(again["id"], "t3", "tab ids are never reused");
}

/// This process's resident set (Linux `VmRSS`), in KB.
fn resident_kb() -> Option<u64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    let line = status.lines().find(|l| l.starts_with("VmRSS:"))?;
    line.split_whitespace().nth(1)?.parse().ok()
}

fn find_ref(run: &mut Run, role: &str, name: &str) -> String {
    let f = run.ok(cmds::FIND, json!({"role": role, "name": name}));
    f["matches"][0]["ref"]
        .as_str()
        .unwrap_or_else(|| panic!("no {role} {name}: {f}"))
        .to_owned()
}

/// The browser's pid, from the process list: the process with this run's profile that is not a child (`--type=`).
fn chrome_pid(profile: &Path) -> u32 {
    let out = std::process::Command::new("ps")
        .args(["-eo", "pid=,args="])
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&out.stdout);
    let needle = format!("--user-data-dir={}", profile.display());
    let line = text
        .lines()
        .find(|l| {
            l.contains(&needle) && l.contains("--remote-debugging-port=0") && !l.contains("--type=")
        })
        .expect("the browser process");
    line.split_whitespace().next().unwrap().parse().unwrap()
}

/// Record `form-axtree.json` from this Chrome (ELUDITE_RECORD_FIXTURES=1).
fn record_ax_tree() {
    let exe = chrome().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let mut engine = ExternalChrome::new(
        EngineConfig {
            executable: Some(exe),
            profile_dir: dir.path().join("p"),
            headless: true,
            viewport: EngineConfig::VIEWPORT,
        },
        ChromeSearch::default(),
        Arc::new(|_: &str| {}),
    )
    .no_sandbox(running_as_root() || eludite_browser::chrome::no_sandbox_from_env());
    let base = serve();
    engine.launch().unwrap();
    let target = engine.open_tab(&format!("{base}/form.html")).unwrap();
    let session = engine.attach(&target).unwrap();
    std::thread::sleep(Duration::from_millis(1000));
    let t = Duration::from_secs(10);
    let tree = engine
        .send(&session, "Accessibility.getFullAXTree", json!({}), t)
        .unwrap();
    let version = engine.info().unwrap().version;
    let doc = json!({"recorded_from": version, "page": "tests/fixtures/form.html", "nodes": tree["nodes"]});
    std::fs::write(
        fixtures().join("form-axtree.json"),
        serde_json::to_string_pretty(&doc).unwrap() + "\n",
    )
    .unwrap();
    println!("recorded form-axtree.json from {version}");
    engine.shutdown();
}

#[test]
fn a_bad_chrome_path_fails_the_launch_and_tabs_launches_nothing() {
    let mut b = Browser::new(
        Box::new(ExternalChrome::new(
            EngineConfig {
                executable: Some(PathBuf::from("/nonexistent/chrome")),
                profile_dir: std::env::temp_dir().join("eludite-browser-test-unused"),
                headless: true,
                viewport: EngineConfig::VIEWPORT,
            },
            ChromeSearch::default(),
            Arc::new(|_: &str| {}),
        )),
        Arc::new(|_: &str| {}),
    );
    match b.apply(BrowserRequest::Tabs).unwrap() {
        BrowserOutput::Tabs(t) => assert!(!t.running),
        other => panic!("{other:?}"),
    }
    let err = b
        .apply(BrowserRequest::TabOpen { url: None })
        .unwrap_err()
        .to_string();
    assert!(err.contains("not an executable"), "{err}");
}
