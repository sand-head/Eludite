//! The browser commands against a real, headless Chrome or Chromium (brief 0023). Skips with a message when none
//! is found (ELUDITE_CHROME, `tools/chrome/fetch.sh`'s cache, PATH, the usual install locations).
//!
//! A std-only HTTP server in this test serves `tests/fixtures/`. One Chrome is launched for the whole run (a launch
//! costs about a second), and the steps run in order: tabs before launch, tab_open, navigate with `load` and
//! `network_idle`, screenshot, read_page, find, page_text, console, network, wait, evaluate, a stale ref after the
//! document is rewritten, tab_close, then Chrome killed under a pending request and relaunched. The budgets' timings
//! are printed (`cargo test -p eludite-browser --test chrome -- --nocapture`).
//!
//! Brief 0024's actions run in a second test against `act.html`, `storage.html` and `fetch.html`: every `input`
//! action by ref and by point, `form_input`, `upload`, a stale ref, console errors, a navigating Enter, `storage` and
//! `network_body`, with their budgets. The two tests take turns ([`ONE_CHROME`]).
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
    let mime_of = |path: &str| {
        if path.ends_with(".html") {
            "text/html; charset=utf-8"
        } else if path.ends_with(".png") {
            "image/png"
        } else if path.ends_with(".txt") {
            "text/plain"
        } else {
            "application/json"
        }
    };
    let (status, body, mime) = if path == BIG_JSON {
        ("200 OK", big_json(), "application/json")
    } else if path == "favicon.ico" {
        // Chrome asks for it after every page; a 404 would land in the console the tests expect empty.
        ("200 OK", Vec::new(), "image/x-icon")
    } else if ok {
        ("200 OK", std::fs::read(&file).unwrap(), mime_of(path))
    } else {
        ("404 Not Found", b"not found".to_vec(), "text/plain")
    };
    // The storage page also gets a cookie from the server, which pages cannot read (HttpOnly).
    let cookie = if path == "storage.html" {
        "Set-Cookie: session_id=abc123; Path=/; HttpOnly; SameSite=Strict\r\n"
    } else {
        ""
    };
    let head = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {mime}\r\nContent-Length: {}\r\nCache-Control: no-store\r\n{cookie}Connection: close\r\n\r\n",
        body.len()
    );
    let _ = stream.write_all(head.as_bytes());
    let _ = stream.write_all(&body);
    let _ = stream.flush();
    let _ = stream.read(&mut [0u8; 1]);
}

/// A 1 MB JSON body the server makes (`network_body`'s budget).
const BIG_JSON: &str = "big.json";
const BIG_BYTES: usize = 1024 * 1024;

fn big_json() -> Vec<u8> {
    let mut s = String::from("{\"data\": \"");
    while s.len() < BIG_BYTES - 2 {
        s.push_str("0123456789abcdef");
    }
    s.truncate(BIG_BYTES - 2);
    s.push_str("\"}");
    s.into_bytes()
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
                allow_no_sandbox: false,
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

/// The Chrome tests run one at a time, so one's timings are not taken while the other's browser works.
static ONE_CHROME: Mutex<()> = Mutex::new(());

#[test]
fn the_commands_against_a_headless_chrome() {
    let _one = ONE_CHROME.lock().unwrap_or_else(|e| e.into_inner());
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
    // Read element by element (the page has more than 1,500 elements): the same rows, depths and boxes.
    let first = &big["nodes"][0];
    assert_eq!(
        (first["role"].as_str(), first["name"].as_str()),
        (Some("link"), Some("Item 1")),
        "{first}"
    );
    assert_eq!(first["depth"], 0);
    assert!(first["box"]["height"].as_f64().unwrap() > 0., "{first}");
    assert_eq!(big["nodes"][499]["name"], "Item 500");
    let by_find = run.ok(
        cmds::FIND,
        json!({"role": "link", "name": "Item 1", "max": 1}),
    );
    assert_eq!(
        by_find["matches"][0]["ref"], first["ref"],
        "the same element keeps its ref"
    );
    let boxed = &by_find["matches"][0]["box"];
    for k in ["x", "y", "width", "height"] {
        assert!(
            (boxed[k].as_f64().unwrap() - first["box"][k].as_f64().unwrap()).abs() <= 0.2,
            "{k}: {boxed} {first}"
        );
    }
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
            allow_no_sandbox: false,
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
                allow_no_sandbox: false,
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

/// `window.events` of the act fixture since the last call (and clears them).
fn take_events(run: &mut Run) -> Vec<String> {
    let v = run.ok(
        cmds::EVALUATE,
        json!({"expression": "window.events.splice(0, window.events.length)"}),
    );
    v["result"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e.as_str().unwrap().to_owned())
        .collect()
}

fn eval(run: &mut Run, expression: &str) -> Value {
    run.ok(cmds::EVALUATE, json!({ "expression": expression }))["result"].clone()
}

fn css_ref(run: &mut Run, css: &str) -> (String, Value) {
    let f = run.ok(cmds::FIND, json!({ "css": css }));
    let m = &f["matches"][0];
    (
        m["ref"]
            .as_str()
            .unwrap_or_else(|| panic!("no {css}: {f}"))
            .to_owned(),
        m["box"].clone(),
    )
}

#[test]
fn acting_on_the_page_in_a_headless_chrome() {
    let _one = ONE_CHROME.lock().unwrap_or_else(|e| e.into_inner());
    let Some(exe) = chrome() else { return };
    let profile = tempfile::tempdir().unwrap();
    let (mut run, _log) = Run::new(&exe, &profile.path().join(".eludite/browser/profile"));
    let act_url = run.url("act.html");
    run.ok(cmds::TAB_OPEN, json!({ "url": act_url }));
    take_events(&mut run);

    // click by ref: the element it landed on, its point, no navigation, no errors.
    let counter = find_ref(&mut run, "button", "Count");
    let out = run.ok(
        cmds::INPUT,
        json!({"action": "click", "ref": counter, "wait_ms": 100}),
    );
    assert_eq!(out["action"], "click");
    assert_eq!(out["target"]["ref"], counter.as_str());
    assert_eq!(out["target"]["role"], "button");
    assert_eq!(out["target"]["name"], "Count");
    assert_eq!(out["navigated"], false);
    assert_eq!(out["console_errors"], json!([]));
    assert!(out["elapsed_ms"].as_f64().unwrap() >= 100., "{out}");
    let point = out["target"]["point"].clone();
    assert_eq!(take_events(&mut run), ["focus:counter", "click:counter:1"]);
    // click by point, with a modifier held: the same element, by the same ref.
    let out = run.ok(
        cmds::INPUT,
        json!({"action": "click", "x": point["x"], "y": point["y"], "modifiers": ["shift"], "wait_ms": 0}),
    );
    assert_eq!(out["target"]["ref"], counter.as_str(), "{out}");
    assert_eq!(take_events(&mut run), ["click:counter:1:shift"]);
    run.ok(
        cmds::INPUT,
        json!({"action": "double_click", "ref": counter, "wait_ms": 0}),
    );
    assert_eq!(
        take_events(&mut run),
        ["click:counter:1", "click:counter:2", "dblclick:counter"]
    );
    run.ok(
        cmds::INPUT,
        json!({"action": "right_click", "ref": counter, "wait_ms": 0}),
    );
    assert_eq!(take_events(&mut run), ["contextmenu:counter"]);

    // type (one insertion, then per key), key with a modifier, focus.
    let name = find_ref(&mut run, "textbox", "Name");
    run.ok(
        cmds::INPUT,
        json!({"action": "type", "ref": name, "text": "Grace", "wait_ms": 0}),
    );
    assert_eq!(
        eval(&mut run, "document.getElementById('name').value"),
        "Grace"
    );
    assert_eq!(take_events(&mut run), ["input:name"]);
    run.ok(
        cmds::INPUT,
        json!({"action": "type", "text": " Hop", "per_key": true, "wait_ms": 0}),
    );
    assert_eq!(
        eval(&mut run, "document.getElementById('name').value"),
        "Grace Hop"
    );
    let typed = take_events(&mut run);
    assert!(typed.contains(&"keydown:H".to_owned()), "{typed:?}");
    run.ok(
        cmds::INPUT,
        json!({"action": "key", "ref": name, "keys": ["Control+a", "Backspace"], "wait_ms": 0}),
    );
    assert_eq!(eval(&mut run, "document.getElementById('name').value"), "");
    run.ok(
        cmds::INPUT,
        json!({"action": "focus", "ref": counter, "wait_ms": 0}),
    );
    assert_eq!(eval(&mut run, "document.activeElement.id"), "counter");

    // hover shows the title.
    let (hover, _) = css_ref(&mut run, "#hoverme");
    run.ok(
        cmds::INPUT,
        json!({"action": "hover", "ref": hover, "wait_ms": 0}),
    );
    assert_eq!(
        eval(&mut run, "document.getElementById('tooltip').textContent"),
        "Tooltip text"
    );

    // select: by label and by value, one and several; a wrong option names the options.
    let (plan, _) = css_ref(&mut run, "#plan");
    let out = run.ok(
        cmds::INPUT,
        json!({"action": "select", "ref": plan, "values": ["Team"], "wait_ms": 0}),
    );
    assert_eq!(out["selected"], json!(["team"]));
    let (extras, _) = css_ref(&mut run, "#extras");
    let out = run.ok(
        cmds::INPUT,
        json!({"action": "select", "ref": extras, "values": ["mug", "Shirt"], "wait_ms": 0}),
    );
    assert_eq!(out["selected"], json!(["mug", "shirt"]));
    let err = run
        .call(
            cmds::INPUT,
            json!({"action": "select", "ref": plan, "values": ["Gold"]}),
        )
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("\"Gold\"") && err.contains("\"pro\" (Pro)"),
        "{err}"
    );
    let ev = take_events(&mut run);
    assert!(ev.contains(&"change:plan".to_owned()) && ev.contains(&"change:extras".to_owned()));

    // drag moves the slider to its right end.
    let (volume, b) = css_ref(&mut run, "#volume");
    let right = b["x"].as_f64().unwrap() + b["width"].as_f64().unwrap() - 1.;
    let mid = b["y"].as_f64().unwrap() + b["height"].as_f64().unwrap() / 2.;
    let out = run.ok(
        cmds::INPUT,
        json!({"action": "drag", "ref": volume, "to": {"x": right, "y": mid}, "wait_ms": 0}),
    );
    assert!(out["to"]["point"].is_object(), "{out}");
    let value: f64 = eval(&mut run, "document.getElementById('volume').value")
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    assert!(value >= 90., "the slider moved to {value}");

    // console_errors carry the error a handler throws, with its location.
    let boom = find_ref(&mut run, "button", "Boom");
    let out = run.ok(
        cmds::INPUT,
        json!({"action": "click", "ref": boom, "wait_ms": 100}),
    );
    let errors = out["console_errors"].as_array().unwrap();
    assert_eq!(errors.len(), 1, "{out}");
    assert!(
        errors[0]["text"]
            .as_str()
            .unwrap()
            .contains("boom from the click handler"),
        "{out}"
    );
    assert_eq!(errors[0]["source"], "exception");
    assert!(errors[0]["url"].as_str().unwrap().ends_with("act.html"));
    assert!(errors[0]["line"].as_u64().unwrap() > 1);

    // scroll moves the viewport.
    let out = run.ok(
        cmds::INPUT,
        json!({"action": "scroll", "delta": {"x": 0, "y": 700}, "wait_ms": 300}),
    );
    assert!(out["target"]["point"].is_object());
    let y = eval(&mut run, "scrollY").as_f64().unwrap();
    assert!(y >= 600., "scrolled to {y}");
    run.ok(
        cmds::INPUT,
        json!({"action": "scroll", "delta": {"y": -5000}, "wait_ms": 300}),
    );
    assert_eq!(eval(&mut run, "scrollY").as_f64().unwrap(), 0.);

    // form_input: text, a radio, checkboxes, a single and a multiple select, a file; a disabled field and a
    // non-field fail alone.
    let dir = tempfile::tempdir().unwrap();
    let upload = dir.path().join("notes.txt");
    std::fs::write(&upload, "hello upload\n").unwrap();
    let upload = upload.to_string_lossy().into_owned();
    let refs: Vec<String> = [
        "#name", "#notes", "#large", "#gift", "#express", "#plan", "#extras", "#file", "#locked",
        "#hoverme",
    ]
    .iter()
    .map(|c| css_ref(&mut run, c).0)
    .collect();
    take_events(&mut run);
    let fields = json!([
        {"ref": refs[0], "value": "Ada Lovelace"},
        {"ref": refs[1], "value": "Leave at the door"},
        {"ref": refs[2], "value": true},
        {"ref": refs[3], "value": true},
        {"ref": refs[4], "value": false},
        {"ref": refs[5], "value": "Pro"},
        {"ref": refs[6], "value": ["stickers", "mug"]},
        {"ref": refs[7], "value": {"files": [upload]}},
        {"ref": refs[8], "value": "x"},
        {"ref": refs[9], "value": "x"},
    ]);
    let out = run.ok(cmds::FORM_INPUT, json!({ "fields": fields }));
    let oks: Vec<bool> = out["fields"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["ok"].as_bool().unwrap())
        .collect();
    assert_eq!(
        oks,
        [true, true, true, true, true, true, true, true, false, false],
        "{out}"
    );
    assert!(
        out["fields"][8]["message"]
            .as_str()
            .unwrap()
            .contains("disabled")
    );
    assert!(
        out["fields"][9]["message"]
            .as_str()
            .unwrap()
            .contains("not a form field")
    );
    let state = eval(
        &mut run,
        "(() => { const $ = (i) => document.getElementById(i); return [$('name').value, $('notes').value, $('large').checked, $('small').checked, $('gift').checked, $('express').checked, $('plan').value, [...$('extras').selectedOptions].map((o) => o.value).join('+'), $('file').files[0].name]; })()",
    );
    assert_eq!(
        state,
        json!([
            "Ada Lovelace",
            "Leave at the door",
            true,
            false,
            true,
            false,
            "pro",
            "stickers+mug",
            "notes.txt"
        ])
    );
    let ev = take_events(&mut run);
    for e in [
        "input:name",
        "change:name",
        "change:large",
        "change:express",
        "change:plan",
    ] {
        assert!(ev.contains(&e.to_owned()), "{e} in {ev:?}");
    }
    // The page reads the file the input holds.
    let waited = run.ok(
        cmds::WAIT,
        json!({"for": "text", "text": "notes.txt (13 bytes): hello upload", "wait_ms": 3000}),
    );
    assert_eq!(waited["timeout"], false, "{waited}");

    // upload sets a file input; the page reads the name; a second file needs `multiple`.
    let other = dir.path().join("photo.txt");
    std::fs::write(&other, "second file").unwrap();
    let other = other.to_string_lossy().into_owned();
    let out = run.ok(
        cmds::UPLOAD,
        json!({"ref": refs[7], "paths": [other.clone()]}),
    );
    assert_eq!(out["count"], 1);
    let waited = run.ok(
        cmds::WAIT,
        json!({"for": "text", "text": "photo.txt (11 bytes): second file", "wait_ms": 3000}),
    );
    assert_eq!(waited["timeout"], false, "{waited}");
    let err = run
        .call(
            cmds::UPLOAD,
            json!({"ref": refs[7], "paths": [upload, other]}),
        )
        .unwrap_err()
        .to_string();
    assert!(err.contains("one file"), "{err}");
    assert!(
        run.call(
            cmds::UPLOAD,
            json!({"ref": refs[0], "paths": ["/etc/hostname"]})
        )
        .unwrap_err()
        .to_string()
        .contains("not a file input")
    );
    assert!(
        run.call(
            cmds::UPLOAD,
            json!({"ref": refs[7], "paths": ["/nonexistent/x.txt"]})
        )
        .unwrap_err()
        .to_string()
        .contains("/nonexistent/x.txt")
    );

    // Submit by clicking; the result appears; its text.
    let submit = find_ref(&mut run, "button", "Submit");
    run.ok(
        cmds::INPUT,
        json!({"action": "click", "ref": submit, "wait_ms": 0}),
    );
    let waited = run.ok(
        cmds::WAIT,
        json!({"for": "selector", "css": "#result", "wait_ms": 3000}),
    );
    let result_ref = waited["satisfied"]["ref"].as_str().unwrap().to_owned();
    let text = run.ok(cmds::PAGE_TEXT, json!({ "root": result_ref }));
    assert_eq!(
        text["text"],
        "Ordered: Ada Lovelace, large, pro, extras stickers+mug, gift true"
    );

    // Budgets: an input click round trip with wait_ms 100; form_input with 10 fields.
    let mut clicks = Vec::new();
    for _ in 0..20 {
        clicks.push(timed(|| {
            run.ok(
                cmds::INPUT,
                json!({"action": "click", "ref": counter, "wait_ms": 100}),
            );
        }));
    }
    let (p50, q95, max) = p95(clicks);
    println!(
        "input click round trip (wait_ms 100), 20 clicks: p50 {p50:.1} ms, p95 {q95:.1} ms, max {max:.1} ms (budget p95 < 150 ms)"
    );
    assert!(q95 < 150., "input click p95 {q95:.1} ms");
    let ten = json!([
        {"ref": refs[0], "value": "A"}, {"ref": refs[1], "value": "B"}, {"ref": refs[2], "value": true},
        {"ref": refs[3], "value": false}, {"ref": refs[4], "value": true}, {"ref": refs[5], "value": "free"},
        {"ref": refs[6], "value": ["shirt"]}, {"ref": refs[0], "value": "Ada"}, {"ref": refs[1], "value": "C"},
        {"ref": refs[3], "value": true},
    ]);
    let mut fills = Vec::new();
    for _ in 0..20 {
        fills.push(timed(|| {
            let out = run.ok(cmds::FORM_INPUT, json!({ "fields": ten }));
            assert!(
                out["fields"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .all(|f| f["ok"] == true)
            );
        }));
    }
    let (p50, q95, max) = p95(fills);
    println!(
        "form_input with 10 fields, 20 calls: p50 {p50:.1} ms, p95 {q95:.1} ms, max {max:.1} ms (budget p95 < 200 ms)"
    );
    assert!(q95 < 200., "form_input p95 {q95:.1} ms");

    // key Enter submits the search form: the page navigates, and refs from before are stale.
    let q = find_ref(&mut run, "searchbox", "Search");
    run.ok(
        cmds::INPUT,
        json!({"action": "type", "ref": q, "text": "cats", "wait_ms": 0}),
    );
    let gen_before = run.ok(cmds::TABS, json!({}))["tabs"][0]["page_generation"]
        .as_u64()
        .unwrap();
    let out = run.ok(
        cmds::INPUT,
        json!({"action": "key", "keys": "Enter", "wait_ms": 5000}),
    );
    assert_eq!(out["navigated"], true, "{out}");
    assert!(
        out["url"].as_str().unwrap().ends_with("act.html?q=cats"),
        "{out}"
    );
    assert!(out["page_generation"].as_u64().unwrap() > gen_before);
    assert!(
        out["elapsed_ms"].as_f64().unwrap() < 4000.,
        "ended at the load: {out}"
    );
    assert_eq!(
        run.ok(cmds::TABS, json!({}))["tabs"][0]["title"],
        "Search: cats"
    );
    let err = run
        .call(cmds::INPUT, json!({"action": "click", "ref": counter}))
        .unwrap_err();
    assert!(matches!(err, CommandError::InvalidInput(_)), "{err}");
    assert!(err.to_string().contains("stale ref"), "{err}");
    assert!(
        run.call(
            cmds::FORM_INPUT,
            json!({"fields": [{"ref": refs[0], "value": "x"}]})
        )
        .unwrap_err()
        .to_string()
        .contains("stale ref")
    );

    // storage: get lists the cookies (the page's and the server's HttpOnly one) and the items; clear empties them.
    let storage_url = run.url("storage.html");
    run.ok(cmds::NAVIGATE, json!({ "url": storage_url }));
    let got = run.ok(cmds::STORAGE, json!({}));
    let origin = run.base.clone();
    assert_eq!(got["origin"], origin.as_str());
    let mut cookies: Vec<(String, String, bool)> = got["cookies"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| {
            (
                c["name"].as_str().unwrap().to_owned(),
                c["value"].as_str().unwrap().to_owned(),
                c["http_only"].as_bool().unwrap(),
            )
        })
        .collect();
    cookies.sort();
    assert_eq!(
        cookies,
        [
            ("session_id".into(), "abc123".into(), true),
            ("theme".into(), "dark".into(), false)
        ]
    );
    let local = got["local"].as_array().unwrap();
    assert_eq!(local.len(), 2, "{got}");
    let long = local.iter().find(|i| i["key"] == "long").unwrap();
    assert_eq!(long["truncated"], true);
    assert_eq!(long["value"].as_str().unwrap().len(), 1000);
    assert_eq!(
        local.iter().find(|i| i["key"] == "cart").unwrap()["value"],
        "{\"items\":3}"
    );
    assert_eq!(got["session"], json!([{"key": "step", "value": "2"}]));
    assert_eq!(got["total"], 5);
    let only = run.ok(cmds::STORAGE, json!({"kind": "session"}));
    assert!(only.get("cookies").is_none() && only["total"] == 1);
    let cleared = run.ok(cmds::STORAGE, json!({"action": "clear"}));
    assert_eq!(
        cleared["cleared"],
        json!({"cookies": 2, "local": 2, "session": 1})
    );
    let after = run.ok(cmds::STORAGE, json!({ "origin": origin }));
    assert_eq!(after["total"], 0, "{after}");
    assert_eq!(
        eval(&mut run, "localStorage.length + sessionStorage.length"),
        0
    );

    // network_body: the fetch's JSON, a binary body as base64, a request Chrome has no body for, an unknown one.
    let fetch_url = run.url("fetch.html");
    run.ok(cmds::NAVIGATE, json!({ "url": fetch_url }));
    run.ok(
        cmds::WAIT,
        json!({"for": "function", "expression": "window.fetched === true", "wait_ms": 5000}),
    );
    let requests = run.ok(cmds::NETWORK, json!({"max": 2000}))["requests"].clone();
    let id_of = |suffix: &str| -> String {
        requests
            .as_array()
            .unwrap()
            .iter()
            .rev()
            .find(|r| r["url"].as_str().unwrap().ends_with(suffix))
            .unwrap_or_else(|| panic!("no request for {suffix}: {requests:#}"))["request_id"]
            .as_str()
            .unwrap()
            .to_owned()
    };
    let body = run.ok(
        cmds::NETWORK_BODY,
        json!({"request_id": id_of("data.json")}),
    );
    assert_eq!(body["status"], 200);
    assert_eq!(body["mime_type"], "application/json");
    assert_eq!(body["headers"]["Content-Type"], "application/json");
    let parsed: Value = serde_json::from_str(body["body"].as_str().unwrap()).unwrap();
    assert_eq!(parsed["message"], "hello from data.json");
    assert_eq!(body["truncated"], false);
    let png = run.ok(
        cmds::NETWORK_BODY,
        json!({"request_id": id_of("pixel.png")}),
    );
    let bytes =
        eludite_browser::browser::base64_decode(png["body_base64"].as_str().unwrap()).unwrap();
    assert_eq!(bytes, std::fs::read(fixtures().join("pixel.png")).unwrap());
    assert_eq!(png["size"], bytes.len());
    assert!(png.get("body").is_none());
    let refused = run
        .call(cmds::NETWORK_BODY, json!({"request_id": id_of("/refused")}))
        .unwrap_err()
        .to_string();
    assert!(refused.contains("has no body for request"), "{refused}");
    assert!(
        run.call(cmds::NETWORK_BODY, json!({"request_id": "no-such-request"}))
            .unwrap_err()
            .to_string()
            .contains("no request `no-such-request`")
    );
    // A 1 MB body: cut at max_bytes by default, whole with a larger max_bytes, timed.
    let len = run.ok(cmds::EVALUATE, json!({"expression": "fetchBig()"}))["result"]
        .as_u64()
        .unwrap();
    assert_eq!(len as usize, BIG_BYTES);
    let requests = run.ok(cmds::NETWORK, json!({"url_pattern": BIG_JSON}))["requests"].clone();
    let big = requests[0]["request_id"].as_str().unwrap().to_owned();
    let cut = run.ok(cmds::NETWORK_BODY, json!({ "request_id": big }));
    assert_eq!(cut["truncated"], true);
    assert_eq!(cut["size"], BIG_BYTES);
    assert_eq!(cut["body"].as_str().unwrap().len(), 65_536);
    let mut reads = Vec::new();
    for _ in 0..20 {
        reads.push(timed(|| {
            let whole = run.ok(
                cmds::NETWORK_BODY,
                json!({"request_id": big, "max_bytes": 2 * BIG_BYTES}),
            );
            assert_eq!(whole["body"].as_str().unwrap().len(), BIG_BYTES);
        }));
    }
    let (p50, q95, max) = p95(reads);
    println!(
        "network_body of a 1 MB body, 20 reads: p50 {p50:.1} ms, p95 {q95:.1} ms, max {max:.1} ms (budget p95 < 200 ms)"
    );
    assert!(q95 < 200., "network_body p95 {q95:.1} ms");

    run.browser.shutdown();
}
