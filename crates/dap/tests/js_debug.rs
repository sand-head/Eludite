//! vscode-js-debug (brief 0038, `protocol/schemas/dap-js-debug.md`): the browser session, its `startDebugging` child
//! session on a second connection, breakpoints in the original `.ts` file and frames with their generated place.
//!
//! - Against the fake js-debug (`fake::listen_js_debug`), always: the reverse request is answered and the child's
//!   handshake runs on its own connection to the same port; a breakpoint in `app.ts` binds and stops; the frames name
//!   `app.ts` and, through the corpus's committed map, `app.js`; the browser session's `disconnect` ends the child.
//! - Against the real js-debug, Node.js and Chrome (skipped without `ELUDITE_JS_DEBUG` or a found js-debug, Node.js 18
//!   or later, and `ELUDITE_CHROME` or Chrome for Testing): the same on the web corpus's page served here, with the
//!   attach to `running` timed (budget 1.5 s).

mod common;

use std::io::{BufRead as _, Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use common::{Recorder, T};
use eludite_dap::attach::{BrowserTarget, browser_attach, js_exception_filters};
use eludite_dap::discovery::{JsDebugSearch, NodeSearch, check_node_version, node_version_output};
use eludite_dap::fake::{self, FakeJsDebug, FakeProgram, FakeStep, FakeVar};
use eludite_dap::session::{self, StartKind, StartPlan};
use eludite_dap::sourcemap::{MapCache, adapt_stack, normalize};
use eludite_dap::transport::{self, AdapterServer, TcpServer};
use eludite_dap::types::{Event, SourceBreakpoint, StackTraceResponse};
use eludite_dap::{ClientEvent, DapClient, ReverseHandler};
use serde_json::{Value, json};

fn wwwroot() -> PathBuf {
    normalize(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../../corpus/web/minimal-api/wwwroot"))
}

/// The page's script as the fake plays it: `onAdd` (app.ts 21 to 25) calls `total` (14, 15, 17), whose loop skips
/// the first item.
fn page_program() -> FakeProgram {
    let ts = wwwroot().join("app.ts").to_string_lossy().into_owned();
    let v = FakeVar::new;
    let items = v("items", "(1) [{…}]", "Array").with_children(vec![
        v("0", "{name: 'item 1', price: 5}", "Object"),
        v("length", "1", "number"),
    ]);
    let local = |extra: Vec<FakeVar>| {
        let mut l = vec![
            v("input", "input#price", "HTMLInputElement"),
            v("price", "5", "number"),
        ];
        l.extend(extra);
        l
    };
    FakeProgram {
        steps: vec![
            FakeStep::new(&ts, 21, "button#add.onAdd", 0, local(vec![])),
            FakeStep::new(
                &ts,
                24,
                "button#add.onAdd",
                0,
                local(vec![v("item", "{name: 'item 1', price: 5}", "Object")]),
            ),
            FakeStep::new(
                &ts,
                25,
                "button#add.onAdd",
                0,
                local(vec![v("item", "{name: 'item 1', price: 5}", "Object")]),
            ),
            FakeStep::new(
                &ts,
                14,
                "total",
                1,
                vec![items.clone(), v("sum", "0", "number")],
            ),
            FakeStep::new(&ts, 17, "total", 1, vec![items, v("sum", "0", "number")]),
            FakeStep::new(
                &ts,
                26,
                "button#add.onAdd",
                0,
                local(vec![v("sum", "0", "number")]),
            ),
        ],
        ..FakeProgram::default()
    }
}

/// A browser session on `server` with a reverse handler that starts each child on a new connection: (the parent's
/// client, its recorder, the children's clients and recorders as they come).
type Children = Arc<
    Mutex<
        Vec<(
            DapClient,
            Recorder,
            std::thread::JoinHandle<Result<session::Started, String>>,
        )>,
    >,
>;

fn browser_session(
    server: Arc<dyn AdapterServer>,
    target: &BrowserTarget,
    web_root: &Path,
    breakpoints: Vec<(String, Vec<SourceBreakpoint>)>,
) -> (
    DapClient,
    Recorder,
    Children,
    Result<session::Started, eludite_dap::DapError>,
) {
    let children: Children = Arc::default();
    let (kids, srv) = (children.clone(), server.clone());
    let bps = breakpoints.clone();
    let reverse: ReverseHandler = Arc::new(move |command, args| {
        if command != "startDebugging" {
            return None;
        }
        let configuration = args["configuration"].clone();
        let kind = StartKind::from_request(args["request"].as_str().unwrap_or_default())?;
        let conn = match srv.connect() {
            Ok(c) => c,
            Err(e) => return Some(Err(e.to_string())),
        };
        let rec = Recorder::default();
        let maps = Arc::new(Mutex::new(MapCache::new()));
        let inner = rec.sink();
        let client = DapClient::start(
            conn,
            Arc::new(move |e| {
                let e = match e {
                    ClientEvent::Response {
                        request_seq,
                        command,
                        result: Ok(mut body),
                    } if command == "stackTrace" => {
                        adapt_stack(&mut body, &mut maps.lock().unwrap());
                        ClientEvent::Response {
                            request_seq,
                            command,
                            result: Ok(body),
                        }
                    }
                    e => e,
                };
                inner(e)
            }),
        );
        let plan = StartPlan {
            adapter_id: "pwa-chrome".into(),
            kind,
            arguments: configuration,
            breakpoints: bps.clone(),
            exception_filters: js_exception_filters(false, true),
            exception_options: Vec::new(),
            function_breakpoints: Vec::new(),
        };
        let c = client.clone();
        let started =
            std::thread::spawn(move || session::start(&c, &plan, T).map_err(|e| e.to_string()));
        kids.lock().unwrap().push((client, rec, started));
        Some(Ok(json!({})))
    });
    let rec = Recorder::default();
    let parent = DapClient::start_with(server.connect().unwrap(), rec.sink(), Some(reverse));
    let plan = browser_attach(target, web_root);
    let start = StartPlan {
        adapter_id: plan.adapter_id.into(),
        kind: StartKind::Attach,
        arguments: plan.arguments,
        breakpoints: Vec::new(),
        exception_filters: Vec::new(),
        exception_options: Vec::new(),
        function_breakpoints: Vec::new(),
    };
    let started = session::start(&parent, &start, T);
    (parent, rec, children, started)
}

fn wait_children(children: &Children, n: usize) {
    let deadline = Instant::now() + T;
    while children.lock().unwrap().len() < n {
        assert!(Instant::now() < deadline, "no child session");
        std::thread::sleep(Duration::from_millis(2));
    }
}

/// The fake js-debug: `startDebugging` is answered and the child attaches on its own connection with the pending
/// target; the breakpoint in `app.ts` binds once the page's script loads and stops on a click; the frames are
/// `app.ts`'s and carry `app.js`'s place from the committed map; disconnecting the browser session ends the child.
#[test]
fn start_debugging_starts_a_child_on_a_second_connection() {
    let fake = fake::listen_js_debug(FakeJsDebug::page(page_program(), "Minimal API", "TARGET-1"))
        .unwrap();
    let server: Arc<dyn AdapterServer> = Arc::new(TcpServer::listening("127.0.0.1", fake.port));
    let ts = wwwroot().join("app.ts").to_string_lossy().into_owned();
    let target = BrowserTarget {
        address: "127.0.0.1".into(),
        port: 9222,
        target_id: Some("TARGET-1".into()),
        url: "http://localhost:5180/".into(),
        title: "Minimal API".into(),
    };
    let bps = vec![(
        ts.clone(),
        vec![SourceBreakpoint {
            line: 14,
            ..Default::default()
        }],
    )];
    let (parent, _prec, children, started) = browser_session(server, &target, &wwwroot(), bps);
    let started = started.expect("the browser session starts");
    assert!(started.capabilities.supports_log_points);
    let filters: Vec<&str> = started
        .capabilities
        .exception_breakpoint_filters
        .iter()
        .map(|f| f.filter.as_str())
        .collect();
    assert_eq!(filters, ["all", "uncaught"]);
    let p = fake.parent().unwrap();
    assert_eq!(p.last("attach").unwrap()["targetId"], "TARGET-1");
    assert!(
        p.wait_for("response:startDebugging", 1, T),
        "{:?}",
        p.commands()
    );
    wait_children(&children, 1);
    let (child, rec, handshake) = children.lock().unwrap().remove(0);
    let child_started = handshake.join().unwrap().expect("the child's handshake");
    // Set before the script loaded: provisional, then bound by a `breakpoint` event.
    assert_eq!(
        child_started.breakpoints[0].1[0].message.as_deref(),
        Some(fake::JS_PROVISIONAL)
    );
    rec.wait_nth(
        1,
        "bound",
        |e| matches!(e, ClientEvent::Event(Event::Breakpoint(b)) if b.breakpoint.verified),
    );
    let c = fake.child(0).unwrap();
    assert_eq!(c.last("attach").unwrap()["__pendingTargetId"], "TARGET-1");
    assert_eq!(
        c.last("setExceptionBreakpoints").unwrap()["filters"],
        json!(["uncaught"])
    );
    fake.trigger();
    let stopped = rec.stopped(1);
    assert_eq!(
        (stopped.reason.as_str(), stopped.thread_id),
        ("breakpoint", Some(0))
    );
    let st: StackTraceResponse = serde_json::from_value(
        child
            .request_wait(
                "stackTrace",
                json!({"threadId": 0, "startFrame": 0, "levels": 20}),
                T,
            )
            .unwrap(),
    )
    .unwrap();
    // The sink was not between request_wait and the answer: map it as the shell's sink does.
    let mut body = serde_json::to_value(&st).unwrap();
    adapt_stack(&mut body, &mut MapCache::new());
    let st: StackTraceResponse = serde_json::from_value(body).unwrap();
    let top = &st.stack_frames[0];
    assert_eq!(
        (top.name.as_str(), top.path(), top.line),
        ("total", Some(ts.as_str()), 14)
    );
    let g = top.generated.as_ref().expect("a generated place");
    assert_eq!(g.path, wwwroot().join("app.js").to_string_lossy());
    assert_eq!(g.line, Some(8), "app.ts 14 is app.js 8");
    assert_eq!(st.stack_frames[1].name, "button#add.onAdd");
    assert_eq!(
        st.stack_frames.last().unwrap().path(),
        None,
        "the task label has no source"
    );
    // Stop on the browser session: the child hears its target is gone and disconnects first.
    let disconnected = std::thread::spawn({
        let parent = parent.clone();
        move || parent.request_wait("disconnect", json!({"terminateDebuggee": false}), T)
    });
    rec.wait_nth(1, "terminated", |e| {
        matches!(e, ClientEvent::Event(Event::Terminated))
    });
    child.request_wait("disconnect", json!({}), T).unwrap();
    disconnected
        .join()
        .unwrap()
        .expect("the browser session's disconnect is answered");
    parent.kill();
}

// ---- the real js-debug ----

/// js-debug, Node.js and Chrome, or why the real tests skip.
struct Real {
    node: PathBuf,
    script: PathBuf,
    chrome: PathBuf,
}

fn real() -> Result<Real, String> {
    let js = std::env::var_os("ELUDITE_JS_DEBUG").filter(|v| !v.is_empty());
    let search = JsDebugSearch {
        configured: js.map(PathBuf::from),
        ..JsDebugSearch::from_env()
    };
    let script = search.find()?.script;
    let (node, _) = NodeSearch {
        configured: std::env::var_os("ELUDITE_NODE").map(PathBuf::from),
        ..NodeSearch::from_env()
    }
    .find()?;
    check_node_version(&node, node_version_output(&node).as_deref())?;
    let chrome = std::env::var_os("ELUDITE_CHROME")
        .map(PathBuf::from)
        .filter(|p| p.is_file())
        .ok_or("ELUDITE_CHROME is not set to a Chrome (tools/chrome/fetch.sh prints one)")?;
    Ok(Real {
        node,
        script,
        chrome,
    })
}

/// A static file server for `dir` (the page, its script and map) on a loopback port.
fn serve_dir(dir: PathBuf) -> u16 {
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for mut s in l.incoming().flatten() {
            let dir = dir.clone();
            std::thread::spawn(move || {
                let mut req = String::new();
                let mut r = std::io::BufReader::new(s.try_clone().unwrap());
                if r.read_line(&mut req).is_err() {
                    return;
                }
                let path = req
                    .split_whitespace()
                    .nth(1)
                    .unwrap_or("/")
                    .trim_start_matches('/');
                let path = if path.is_empty() { "index.html" } else { path };
                let body = std::fs::read(dir.join(path));
                let reply = match body {
                    Ok(b) => {
                        let ty = match Path::new(path).extension().and_then(|e| e.to_str()) {
                            Some("html") => "text/html",
                            Some("js") => "text/javascript",
                            _ => "application/json",
                        };
                        let mut v = format!(
                            "HTTP/1.1 200 OK\r\nContent-Type: {ty}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                            b.len()
                        )
                        .into_bytes();
                        v.extend(b);
                        v
                    }
                    Err(_) => {
                        b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                            .to_vec()
                    }
                };
                let _ = s.write_all(&reply);
            });
        }
    });
    port
}

/// `GET /json/list` of a DevTools port (HTTP/1.1: Chrome's server keeps the connection, so the body is read by its
/// length).
fn json_list(port: u16) -> Value {
    let Ok(mut s) = std::net::TcpStream::connect(("127.0.0.1", port)) else {
        return Value::Null;
    };
    s.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
    s.write_all(b"GET /json/list HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n")
        .unwrap();
    let mut r = std::io::BufReader::new(s);
    let mut length = 0usize;
    loop {
        let mut line = String::new();
        if r.read_line(&mut line).unwrap_or(0) == 0 {
            return Value::Null;
        }
        let line = line.trim_end();
        if line.is_empty() {
            break;
        }
        if let Some(v) = line.to_ascii_lowercase().strip_prefix("content-length:") {
            length = v.trim().parse().unwrap_or(0);
        }
    }
    let mut body = vec![0u8; length];
    if r.read_exact(&mut body).is_err() {
        return Value::Null;
    }
    serde_json::from_slice(&body).unwrap_or(Value::Null)
}

/// Chrome headless on `url` with a remote debugging port: the process, the port and the page's target id.
fn chrome(exe: &Path, profile: &Path, url: &str) -> (std::process::Child, u16, String) {
    let mut cmd = std::process::Command::new(exe);
    cmd.args([
        "--headless=new",
        "--remote-debugging-port=0",
        &format!("--user-data-dir={}", profile.display()),
        "--no-first-run",
        "--no-default-browser-check",
        url,
    ]);
    if std::env::var("ELUDITE_CHROME_NO_SANDBOX").is_ok_and(|v| v == "1") {
        cmd.arg("--no-sandbox");
    }
    let mut child = cmd
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let mut lines = std::io::BufReader::new(child.stderr.take().unwrap()).lines();
    let ws = lines
        .by_ref()
        .map_while(Result::ok)
        .find_map(|l| l.split("DevTools listening on ").nth(1).map(str::to_owned))
        .expect("Chrome's DevTools endpoint");
    std::thread::spawn(move || for _ in lines {});
    let port: u16 = ws
        .split(':')
        .nth(2)
        .unwrap()
        .split('/')
        .next()
        .unwrap()
        .parse()
        .unwrap();
    let deadline = Instant::now() + T;
    let target = loop {
        let list = json_list(port);
        let page = list
            .as_array()
            .into_iter()
            .flatten()
            .find(|t| t["type"] == "page" && t["url"] == url)
            .cloned();
        if let Some(p) = page {
            break p["id"].as_str().unwrap().to_owned();
        }
        assert!(Instant::now() < deadline, "no page target at {url}: {list}");
        std::thread::sleep(Duration::from_millis(50));
    };
    (child, port, target)
}

/// The real js-debug against Chrome on the web corpus's page: attach by target id to `running` within 1.5 s, the
/// child session from `startDebugging`, a breakpoint in `app.ts` (line 25 of the Add button's handler, where it calls
/// `total`) stops at the mapped line on a click, the Locals of the handler's frame hold its variables, and the frame
/// carries `app.js`'s place.
#[test]
fn the_real_js_debug_stops_in_app_ts_on_a_click() {
    let real = match real() {
        Ok(r) => r,
        Err(e) => {
            eprintln!("skipped: {e}");
            return;
        }
    };
    let t = tempfile::tempdir().unwrap();
    let www = t.path().join("www");
    std::fs::create_dir_all(&www).unwrap();
    for f in ["app.ts", "app.js", "app.js.map"] {
        std::fs::copy(wwwroot().join(f), www.join(f)).unwrap();
    }
    std::fs::write(
        www.join("index.html"),
        "<!doctype html><html><head><title>Minimal API</title></head><body>\
         <input id=\"price\" value=\"5\"><button id=\"add\">Add</button><span id=\"total\"></span>\
         <script src=\"/app.js\"></script></body></html>",
    )
    .unwrap();
    let www = normalize(&www.canonicalize().unwrap());
    let url = format!("http://127.0.0.1:{}/index.html", serve_dir(www.clone()));
    let (mut browser, port, target_id) = chrome(&real.chrome, &t.path().join("profile"), &url);
    let t0 = Instant::now();
    let args = vec![
        real.script.to_string_lossy().into_owned(),
        "0".into(),
        "127.0.0.1".into(),
    ];
    let server: Arc<dyn AdapterServer> = Arc::new(
        transport::start_tcp_server(&real.node, &args, transport::TCP_SERVER_START_TIMEOUT)
            .unwrap(),
    );
    let ts = www.join("app.ts").to_string_lossy().into_owned();
    let target = BrowserTarget {
        address: "127.0.0.1".into(),
        port,
        target_id: Some(target_id),
        url: url.clone(),
        title: "Minimal API".into(),
    };
    let bps = vec![(
        ts.clone(),
        vec![SourceBreakpoint {
            line: 25,
            ..Default::default()
        }],
    )];
    let (parent, _prec, children, started) = browser_session(server, &target, &www, bps);
    started.expect("the browser session attaches");
    wait_children(&children, 1);
    let (child, rec, handshake) = children.lock().unwrap().remove(0);
    handshake.join().unwrap().expect("the child attaches");
    let attach = t0.elapsed();
    eprintln!(
        "timing: js-debug started and attached to running in {:.0} ms",
        attach.as_secs_f64() * 1e3
    );
    // The budget (1.5 s), asserted on a quiet machine only: other builds inflate Node's and Chrome's start.
    let cores = std::thread::available_parallelism().map_or(1, |n| n.get()) as f64;
    let load = std::fs::read_to_string("/proc/loadavg")
        .ok()
        .and_then(|t| t.split_whitespace().next()?.parse::<f64>().ok())
        .unwrap_or(0.0);
    if load <= cores {
        assert!(attach < Duration::from_millis(1500), "{attach:?}");
    }
    rec.wait_nth(
        1,
        "bound",
        |e| matches!(e, ClientEvent::Event(Event::Breakpoint(b)) if b.breakpoint.verified),
    );
    // The click (the shell sends it with eludite.browser.input; here the page's own click()).
    child
        .request_wait(
            "evaluate",
            json!({"expression": "setTimeout(() => document.getElementById('add').click(), 10)", "context": "repl"}),
            T,
        )
        .unwrap();
    let stopped = rec.stopped(1);
    assert_eq!(stopped.reason, "breakpoint");
    let mut body = child
        .request_wait(
            "stackTrace",
            json!({"threadId": stopped.thread_id, "startFrame": 0, "levels": 20}),
            T,
        )
        .unwrap();
    adapt_stack(&mut body, &mut MapCache::new());
    let st: StackTraceResponse = serde_json::from_value(body).unwrap();
    let handler = &st.stack_frames[0];
    assert_eq!(
        (handler.name.as_str(), handler.path(), handler.line),
        ("button#add.onAdd", Some(ts.as_str()), 25)
    );
    let g = handler.generated.as_ref().expect("app.js's place");
    assert_eq!(
        (g.path.as_str(), g.line),
        (www.join("app.js").to_str().unwrap(), Some(18))
    );
    let scopes = child
        .request_wait("scopes", json!({"frameId": handler.id}), T)
        .unwrap();
    let local = scopes["scopes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["name"] == "Local: onAdd")
        .cloned()
        .unwrap();
    let vars = child
        .request_wait(
            "variables",
            json!({"variablesReference": local["variablesReference"]}),
            T,
        )
        .unwrap();
    let names: Vec<&str> = vars["variables"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|v| v["name"].as_str())
        .collect();
    for n in ["input", "price", "item"] {
        assert!(names.contains(&n), "{names:?}");
    }
    child
        .request_wait("continue", json!({"threadId": stopped.thread_id}), T)
        .unwrap();
    let disconnected = std::thread::spawn({
        let parent = parent.clone();
        move || parent.request_wait("disconnect", json!({"terminateDebuggee": false}), T)
    });
    rec.wait_nth(1, "terminated", |e| {
        matches!(e, ClientEvent::Event(Event::Terminated))
    });
    child.request_wait("disconnect", json!({}), T).unwrap();
    disconnected.join().unwrap().unwrap();
    parent.kill();
    let _ = browser.kill();
    let _ = browser.wait();
}
