//! The control protocol against the real engine (brief 0031): create a tab, navigate to data URLs, receive frames
//! with the expected pixels at known points through the shared-memory ring, resize, a CDP round trip, mouse and key
//! input, close and shutdown. Brief 0032: no request on `about:blank` (a net log), popups as tabs, `<select>`
//! popups in the frames, cursors, the context menu, JavaScript dialogs, file choosers, permission and
//! authentication prompts answered by the shell, downloads and their limit, DevTools as a tab, IME, history and the
//! favicon. Needs the `cef` feature (CEF fetched by tools/cef/fetch.sh and CEF_PATH set); skips
//! with a message otherwise. Runs as root only with ELUDITE_CHROME_NO_SANDBOX=1, which this test sets when the
//! effective user is root (as brief 0023's Chrome tests do), saying so.

#![cfg(target_os = "linux")]
// Descriptor 3 for the child, the received descriptors and the effective user: system calls, each commented.
#![allow(unsafe_code)]

use std::collections::HashMap;
use std::io::{BufReader, Write};
use std::os::fd::{AsRawFd, OwnedFd};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use eludite_chromium::shm::{self, Reader};
use serde_json::{Value, json};

struct Engine {
    child: Child,
    stdin: ChildStdin,
    rx: mpsc::Receiver<Value>,
    regions: HashMap<u64, Reader>,
    next_id: u64,
    /// Notifications received while waiting for something else.
    seen: Vec<Value>,
    _profile: tempfile::TempDir,
}

fn spawn_engine() -> Option<(Engine, Duration)> {
    spawn_engine_with(&[], json!({}))
}

/// The engine with extra command-line switches and `initialize` members.
fn spawn_engine_with(extra: &[String], init_extra: Value) -> Option<(Engine, Duration)> {
    if !cfg!(feature = "cef") {
        eprintln!(
            "skipped: eludite-chromium was built without the cef feature (tools/cef/fetch.sh, then CEF_PATH=... \
             cargo test -p eludite-chromium --features cef)"
        );
        return None;
    }
    let exe = env!("CARGO_BIN_EXE_eludite-chromium");
    let profile = tempfile::tempdir().unwrap();
    let profile_path = profile.path().to_path_buf();
    let (ours, theirs) = shm::socket_pair().unwrap();
    let theirs_fd = theirs.as_raw_fd();
    let mut cmd = Command::new(exe);
    cmd.arg("--profile")
        .arg(&profile_path)
        .args(["--frame-socket", "3"])
        .args(extra)
        // More switches for a run by hand (`--vmodule=...` to see what a service does).
        .args(
            std::env::var("ELUDITE_ENGINE_TEST_ARGS")
                .unwrap_or_default()
                .split_whitespace(),
        )
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    // SAFETY: geteuid has no preconditions.
    if unsafe { libc::geteuid() } == 0 {
        eprintln!("running as root: setting ELUDITE_CHROME_NO_SANDBOX=1 for the engine");
        cmd.env("ELUDITE_CHROME_NO_SANDBOX", "1");
    }
    {
        use std::os::unix::process::CommandExt;
        // SAFETY: dup2 is async-signal-safe; it puts the socket at descriptor 3 in the child only (dup2 clears
        // close-on-exec on the copy).
        unsafe {
            cmd.pre_exec(move || {
                if libc::dup2(theirs_fd, 3) < 0 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
    }
    let t0 = Instant::now();
    let mut child = cmd.spawn().expect("start eludite-chromium");
    drop(theirs);
    let stdout = child.stdout.take().unwrap();
    let stderr = child.stderr.take().unwrap();
    std::thread::spawn(move || {
        use std::io::BufRead;
        for line in BufReader::new(stderr).lines().map_while(Result::ok) {
            if !line.contains("ssl_client_socket") && !line.contains("dbus") {
                eprintln!("[engine] {line}");
            }
        }
    });
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || reader(stdout, ours, tx));
    let stdin = child.stdin.take().unwrap();
    let mut e = Engine {
        child,
        stdin,
        rx,
        regions: HashMap::new(),
        next_id: 0,
        seen: Vec::new(),
        _profile: profile,
    };
    let mut params =
        json!({"clientName": "engine-test", "clientVersion": "0", "protocolVersion": 1});
    if let (Some(p), Some(more)) = (params.as_object_mut(), init_extra.as_object()) {
        p.extend(more.clone());
    }
    let init = e.request("initialize", params);
    let init = match init {
        Ok(v) => v,
        Err(err) => {
            let status = e.child.wait().ok();
            panic!("initialize failed ({err}); the engine exited with {status:?}");
        }
    };
    let cold = t0.elapsed();
    assert_eq!(init["engineName"], "eludite-chromium");
    assert_eq!(init["protocolVersion"], 1);
    assert_eq!(init["chromiumVersion"], "154.0.8037.58");
    assert_eq!(init["frameTransport"], "memfd+scm_rights");
    Some((e, cold))
}

/// Framed messages from stdout; a `tab/resized` carries its region's descriptor (received here, in order).
fn reader(stdout: std::process::ChildStdout, sock: OwnedFd, tx: mpsc::Sender<Value>) {
    let mut r = BufReader::new(stdout);
    while let Ok(Some(body)) = eludite_protocol::framing::read_message(&mut r) {
        let mut v: Value = serde_json::from_slice(&body).unwrap();
        if v["method"] == "tab/resized" {
            let (id, fd) = shm::recv_fd(sock.as_raw_fd()).unwrap();
            assert_eq!(json!(id), v["params"]["region"]["id"]);
            // Hand the descriptor over as a number; the test side takes ownership.
            v["fd"] = json!(std::os::fd::IntoRawFd::into_raw_fd(fd));
        }
        if tx.send(v).is_err() {
            return;
        }
    }
}

impl Engine {
    fn send(&mut self, v: Value) {
        let body = serde_json::to_vec(&v).unwrap();
        eludite_protocol::framing::write_message(&mut self.stdin, &body).unwrap();
        self.stdin.flush().unwrap();
    }

    fn notify(&mut self, method: &str, params: Value) {
        self.send(json!({"jsonrpc": "2.0", "method": method, "params": params}));
    }

    fn request(&mut self, method: &str, params: Value) -> Result<Value, Value> {
        self.next_id += 1;
        let id = self.next_id;
        self.send(json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}));
        let m = self.wait(&format!("answer to {method}"), |m| m["id"] == json!(id));
        match m.get("error") {
            Some(e) => Err(e.clone()),
            None => Ok(m["result"].clone()),
        }
    }

    /// The next message matching `pred` (within 20 s); regions are mapped as they arrive.
    fn wait(&mut self, what: &str, mut pred: impl FnMut(&Value) -> bool) -> Value {
        if let Some(i) = self.seen.iter().position(&mut pred) {
            return self.seen.remove(i);
        }
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            let m = self
                .rx
                .recv_timeout(left)
                .unwrap_or_else(|_| panic!("no {what} within 20 s"));
            if m["method"] == "tab/resized" {
                let fd = m["fd"].as_i64().unwrap() as i32;
                // SAFETY: the reader thread received this descriptor and gave up ownership.
                let fd = unsafe { <OwnedFd as std::os::fd::FromRawFd>::from_raw_fd(fd) };
                let reader = Reader::open(fd.as_raw_fd()).unwrap();
                self.regions
                    .insert(m["params"]["region"]["id"].as_u64().unwrap(), reader);
            }
            if pred(&m) {
                return m;
            }
            // Frames are not kept: a later wait wants a newer one.
            if m.get("id").is_none() && m["method"] != "tab/frame" {
                self.seen.push(m);
            }
        }
    }

    /// BGRA at (x, y) device pixels of the newest frame of `region`.
    fn pixel(&self, region: u64, x: u32, y: u32) -> Option<[u8; 4]> {
        self.regions[&region].peek(|m, px| {
            let o = (y * m.stride + x * 4) as usize;
            [px[o], px[o + 1], px[o + 2], px[o + 3]]
        })
    }

    /// Wait for a frame of `tab` whose pixels at the points are the colors.
    fn wait_for_pixels(&mut self, tab: &str, points: &[(u32, u32, [u8; 4])]) -> Value {
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            let f = self.wait("a frame", |m| {
                m["method"] == "tab/frame" && m["params"]["tab"] == tab
            });
            let region = f["params"]["region"].as_u64().unwrap();
            let got: Vec<_> = points
                .iter()
                .map(|&(x, y, _)| self.pixel(region, x, y))
                .collect();
            if points
                .iter()
                .zip(&got)
                .all(|(p, g)| g.as_ref() == Some(&p.2))
            {
                return f;
            }
            assert!(
                Instant::now() < deadline,
                "pixels {got:?} never became {points:?}"
            );
        }
    }
}

const BLUE: [u8; 4] = [255, 0, 0, 255];
const GREEN: [u8; 4] = [0, 128, 0, 255];
const RED: [u8; 4] = [0, 0, 255, 255];
const BLACK: [u8; 4] = [0, 0, 0, 255];
const YELLOW: [u8; 4] = [0, 255, 255, 255];

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

const QUADRANTS: &str = "<html><head><title>quadrants</title><style>html,body{margin:0;height:100%}\
    div{position:absolute;width:50%;height:50%}</style></head><body>\
    <div style='left:0;top:0;background:#f00'></div><div style='right:0;top:0;background:#008000'></div>\
    <div style='left:0;bottom:0;background:#00f'></div><div style='right:0;bottom:0;background:#000'></div>\
    </body></html>";

/// A page that turns yellow when clicked and blue on the key A.
const INPUT: &str = "<html><body style='margin:0;height:100vh;background:#000'>\
    <script>addEventListener('mousedown',()=>document.body.style.background='#ff0');\
    addEventListener('keydown',e=>{if(e.keyCode==65)document.body.style.background='#00f'});</script>\
    </body></html>";

#[test]
fn a_tab_renders_into_the_ring_and_answers_cdp_and_input() {
    let Some((mut e, cold)) = spawn_engine() else {
        return;
    };
    eprintln!("engine cold start to initialize: {cold:?}");

    // Create: a region before the answer, then frames with the four colors at known points.
    let t_open = Instant::now();
    let tab = e
        .request(
            "tab/create",
            json!({"url": data_url(QUADRANTS), "width": 400, "height": 300}),
        )
        .unwrap()["tab"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(tab, "1");
    assert_eq!(e.regions.len(), 1, "tab/resized came before the answer");
    let f = e.wait_for_pixels(
        &tab,
        &[
            (100, 75, RED),
            (300, 75, GREEN),
            (100, 225, BLUE),
            (300, 225, BLACK),
        ],
    );
    eprintln!("tab/create to the expected pixels: {:?}", t_open.elapsed());
    let p = &f["params"];
    assert_eq!(
        (p["width"].as_u64(), p["height"].as_u64()),
        (Some(400), Some(300))
    );
    assert!(p["sequence"].as_u64().unwrap() >= 1);
    assert!(p["paintNs"].as_u64().unwrap() <= shm::monotonic_ns());
    assert!(!p["dirty"].as_array().unwrap().is_empty());
    let first_region = p["region"].as_u64().unwrap();
    let title = e.wait("the title", |m| {
        m["method"] == "tab/state" && m["params"]["title"] == "quadrants"
    });
    assert_eq!(title["params"]["tab"], json!(tab));

    // CDP round trip through the tab's DevTools agent.
    e.notify(
        "tab/cdp",
        json!({"tab": tab, "message": {"id": 7, "method": "Runtime.evaluate",
            "params": {"expression": "document.title + ':' + innerWidth", "returnByValue": true}}}),
    );
    let r = e.wait("the CDP answer", |m| {
        m["method"] == "tab/cdpEvent" && m["params"]["message"]["id"] == 7
    });
    assert_eq!(
        r["params"]["message"]["result"]["result"]["value"],
        "quadrants:400"
    );
    // A CDP message for a tab that does not exist is answered with an error.
    e.notify(
        "tab/cdp",
        json!({"tab": "99", "message": {"id": 8, "method": "Runtime.evaluate", "params": {}}}),
    );
    let r = e.wait("the CDP error", |m| {
        m["method"] == "tab/cdpEvent" && m["params"]["message"]["id"] == 8
    });
    assert_eq!(r["params"]["message"]["error"]["code"], -32000);

    // Resize past the slot size: a new region, frames of the new size, the layout follows.
    e.request(
        "tab/resize",
        json!({"tab": tab, "width": 500, "height": 400}),
    )
    .unwrap();
    let f = e.wait_for_pixels(&tab, &[(125, 100, RED), (375, 300, BLACK)]);
    assert_eq!(f["params"]["width"], 500);
    assert_ne!(f["params"]["region"].as_u64().unwrap(), first_region);
    assert_eq!(e.regions.len(), 2);

    // Navigate, then input: a click and the key A.
    e.request("tab/navigate", json!({"tab": tab, "url": data_url(INPUT)}))
        .unwrap();
    e.wait_for_pixels(&tab, &[(250, 200, BLACK)]);
    for t in ["mouseMove", "mouseDown", "mouseUp"] {
        e.notify(
            "tab/input",
            json!({"tab": tab, "event": {"type": t, "x": 250, "y": 200, "button": "left", "clickCount": 1}}),
        );
    }
    e.wait_for_pixels(&tab, &[(250, 200, YELLOW)]);
    for t in ["rawKeyDown", "keyUp"] {
        e.notify(
            "tab/input",
            json!({"tab": tab, "event": {"type": t, "windowsKeyCode": 65}}),
        );
    }
    e.wait_for_pixels(&tab, &[(250, 200, BLUE)]);

    // Errors.
    let err = e
        .request("tab/navigate", json!({"tab": "99", "url": "about:blank"}))
        .unwrap_err();
    assert_eq!(err["code"], -32001);
    let err = e.request("tab/fly", json!({})).unwrap_err();
    assert_eq!(err["code"], -32601);

    // Close and shut down.
    e.request("tab/close", json!({"tab": tab})).unwrap();
    let c = e.wait("tab/closed", |m| m["method"] == "tab/closed");
    assert_eq!(c["params"]["reason"], "closed");
    e.request("shutdown", json!({})).unwrap();
    let status = e.child.wait().unwrap();
    assert!(status.success(), "{status:?}");
}

#[test]
fn a_closed_stdin_shuts_the_engine_down() {
    let Some((e, _)) = spawn_engine() else {
        return;
    };
    let Engine {
        mut child, stdin, ..
    } = e;
    drop(stdin);
    let t = Instant::now();
    loop {
        if let Some(s) = child.try_wait().unwrap() {
            assert!(s.success(), "{s:?}");
            break;
        }
        assert!(t.elapsed() < Duration::from_secs(10), "still running");
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// What a Chromium net log says left the process: urls requested, hosts resolved, sockets connected.
fn net_log_requests(path: &std::path::Path) -> Vec<String> {
    let mut text =
        std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    // A log cut off by the exit lacks its closing brackets.
    let parsed: Value = match serde_json::from_str(&text) {
        Ok(v) => v,
        Err(_) => {
            let t = text.trim_end().trim_end_matches(',').to_owned();
            text = t + "]}";
            serde_json::from_str(&text).expect("the net log parses")
        }
    };
    let types: HashMap<u64, String> = parsed["constants"]["logEventTypes"]
        .as_object()
        .unwrap()
        .iter()
        .map(|(k, v)| (v.as_u64().unwrap(), k.clone()))
        .collect();
    let mut out = Vec::new();
    for e in parsed["events"].as_array().unwrap() {
        let kind = types
            .get(&e["type"].as_u64().unwrap_or(u64::MAX))
            .cloned()
            .unwrap_or_default();
        let p = &e["params"];
        let url = p["url"]
            .as_str()
            .or(p["original_url"].as_str())
            .unwrap_or("");
        let network_url = ["http:", "https:", "ws:", "wss:", "ftp:"]
            .iter()
            .any(|s| url.starts_with(s));
        let connects = matches!(
            kind.as_str(),
            "TCP_CONNECT"
                | "HOST_RESOLVER_MANAGER_REQUEST"
                | "HOST_RESOLVER_DNS_TASK"
                | "QUIC_SESSION"
                | "UDP_CONNECT"
                | "SSL_CONNECT"
        );
        if network_url || connects {
            let what = if !url.is_empty() {
                url.to_owned()
            } else {
                p["host"]
                    .as_str()
                    .or(p["address"].as_str())
                    .or(p["host_and_port"].as_str())
                    .map(str::to_owned)
                    .unwrap_or_else(|| p.to_string())
            };
            let annotation = p["traffic_annotation"]
                .as_i64()
                .map(|a| format!(" (annotation {a})"))
                .unwrap_or_default();
            out.push(format!("{kind}: {what}{annotation}"));
        }
    }
    out.sort();
    out.dedup();
    out
}

#[test]
fn the_engine_makes_no_request_on_about_blank() {
    let dir = tempfile::tempdir().unwrap();
    // ELUDITE_NET_LOG_KEEP names a file to keep the log in, to read by hand.
    let log = std::env::var_os("ELUDITE_NET_LOG_KEEP")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| dir.path().join("net-log.json"));
    let Some((mut e, _)) = spawn_engine_with(
        &[
            format!("--log-net-log={}", log.display()),
            "--net-log-capture-mode=Everything".into(),
        ],
        json!({}),
    ) else {
        return;
    };
    let tab = e
        .request(
            "tab/create",
            json!({"url": "about:blank", "width": 400, "height": 300}),
        )
        .unwrap()["tab"]
        .as_str()
        .unwrap()
        .to_owned();
    e.wait("a frame", |m| {
        m["method"] == "tab/frame" && m["params"]["tab"] == tab
    });
    // Ten seconds: Chromium's background services start within the first few after the profile loads.
    let until = Instant::now() + Duration::from_secs(10);
    while Instant::now() < until {
        let _ = e.rx.recv_timeout(Duration::from_millis(200));
    }
    e.request("shutdown", json!({})).unwrap();
    let status = e.child.wait().unwrap();
    assert!(status.success(), "{status:?}");
    let requests = net_log_requests(&log);
    eprintln!(
        "net log: {} bytes, requests: {requests:#?}",
        std::fs::metadata(&log).unwrap().len()
    );
    assert!(
        requests.is_empty(),
        "the engine reached the network on about:blank: {requests:#?}"
    );
}

// ---- the Web Browser window's methods (brief 0032) ----

/// A tiny HTTP server for pages that need a real origin (permissions, popups that touch their opener, downloads,
/// authentication): `GET /path` answers the route's body; `/auth` asks for Basic credentials until it gets `u:p`.
struct Site {
    base: String,
}

fn site(routes: &[(&str, &str, &[u8])]) -> Site {
    let routes: Vec<(String, String, Vec<u8>)> = routes
        .iter()
        .map(|(p, t, b)| ((*p).to_owned(), (*t).to_owned(), b.to_vec()))
        .collect();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { break };
            let routes = routes.clone();
            std::thread::spawn(move || {
                use std::io::BufRead;
                let mut r = BufReader::new(stream.try_clone().unwrap());
                let mut first = String::new();
                if r.read_line(&mut first).is_err() {
                    return;
                }
                let mut headers = Vec::new();
                loop {
                    let mut l = String::new();
                    if r.read_line(&mut l).is_err() || l.trim().is_empty() {
                        break;
                    }
                    headers.push(l.trim().to_owned());
                }
                let path = first.split_whitespace().nth(1).unwrap_or("/").to_owned();
                let path = path.split('?').next().unwrap_or("/").to_owned();
                let authorized = headers
                    .iter()
                    .any(|h| h.eq_ignore_ascii_case("authorization: basic dTpw"));
                let (status, extra, ty, body) = if path == "/auth" && !authorized {
                    (
                        "401 Unauthorized",
                        "WWW-Authenticate: Basic realm=\"eludite-test\"\r\n",
                        "text/html",
                        b"no".to_vec(),
                    )
                } else if let Some((_, t, b)) = routes.iter().find(|(p, _, _)| *p == path) {
                    let extra = if t == "application/octet-stream" {
                        "Content-Disposition: attachment; filename=\"report.bin\"\r\n"
                    } else {
                        ""
                    };
                    ("200 OK", extra, t.as_str(), b.clone())
                } else {
                    ("404 Not Found", "", "text/plain", b"missing".to_vec())
                };
                let head = format!(
                    "HTTP/1.1 {status}\r\n{extra}Content-Type: {ty}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = stream.write_all(head.as_bytes());
                let _ = stream.write_all(&body);
            });
        }
    });
    Site { base }
}

impl Site {
    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.base)
    }
}

impl Engine {
    fn open(&mut self, url: &str, w: u32, h: u32) -> String {
        self.request("tab/create", json!({"url": url, "width": w, "height": h}))
            .unwrap()["tab"]
            .as_str()
            .unwrap()
            .to_owned()
    }

    fn title_is(&mut self, tab: &str, title: &str) {
        let t = title.to_owned();
        let tab = tab.to_owned();
        self.wait(&format!("the title {title}"), |m| {
            m["method"] == "tab/state" && m["params"]["tab"] == tab && m["params"]["title"] == t
        });
    }

    /// The page has been laid out and painted (two animation frames), so input lands on its content.
    fn painted(&mut self, tab: &str) {
        let expr = "new Promise(r => requestAnimationFrame(() => requestAnimationFrame(() => r(document.readyState))))";
        self.next_id += 1;
        let id = 10_000 + self.next_id;
        self.notify(
            "tab/cdp",
            json!({"tab": tab, "message": {"id": id, "method": "Runtime.evaluate",
                "params": {"expression": expr, "awaitPromise": true, "returnByValue": true}}}),
        );
        self.wait("two animation frames", |m| {
            m["method"] == "tab/cdpEvent" && m["params"]["message"]["id"] == id
        });
    }

    fn click(&mut self, tab: &str, x: i64, y: i64, button: &str) {
        self.painted(tab);
        for t in ["mouseMove", "mouseDown", "mouseUp"] {
            self.notify(
                "tab/input",
                json!({"tab": tab, "event": {"type": t, "x": x, "y": y, "button": button, "clickCount": 1}}),
            );
        }
    }

    fn evaluate(&mut self, tab: &str, expression: &str) -> Value {
        self.next_id += 1;
        let id = 10_000 + self.next_id;
        self.notify(
            "tab/cdp",
            json!({"tab": tab, "message": {"id": id, "method": "Runtime.evaluate",
                "params": {"expression": expression, "returnByValue": true}}}),
        );
        let r = self.wait("the evaluation", |m| {
            m["method"] == "tab/cdpEvent" && m["params"]["message"]["id"] == id
        });
        r["params"]["message"]["result"]["result"]["value"].clone()
    }
}

/// A page whose whole body is a button (so a click anywhere lands on it).
fn button_page(title: &str, onclick: &str) -> String {
    format!(
        "<html><head><title>{title}</title></head><body style='margin:0'>\
         <button style='width:100vw;height:100vh' onclick=\"{onclick}\">go</button></body></html>"
    )
}

#[test]
fn popups_become_tabs_and_keep_their_opener() {
    let s = site(&[
        ("/", "text/html", button_page("opener", "window.open('/child')").as_bytes()),
        (
            "/child",
            "text/html",
            b"<html><head><title>child</title></head><body><script>window.opener.document.title='opened'</script></body></html>",
        ),
    ]);
    let Some((mut e, _)) = spawn_engine() else {
        return;
    };
    let tab = e.open(&s.url("/"), 400, 300);
    e.title_is(&tab, "opener");
    e.click(&tab, 200, 150, "left");
    let popup = e.wait("tab/popup", |m| m["method"] == "tab/popup");
    let child = popup["params"]["tab"].as_str().unwrap().to_owned();
    assert_eq!(popup["params"]["opener"], json!(tab));
    assert!(
        popup["params"]["url"].as_str().unwrap().ends_with("/child"),
        "{popup}"
    );
    assert_eq!(popup["params"]["userGesture"], true);
    assert_ne!(child, tab);
    // Its region came first; it paints and has its own state; its page reached its opener.
    e.wait("a frame of the popup", |m| {
        m["method"] == "tab/frame" && m["params"]["tab"] == child
    });
    e.title_is(&child, "child");
    e.title_is(&tab, "opened");
    // It answers CDP as a tab, and closes as one.
    assert_eq!(e.evaluate(&child, "location.pathname"), "/child");
    e.request("tab/close", json!({"tab": child})).unwrap();
    e.wait("the popup closed", |m| {
        m["method"] == "tab/closed" && m["params"]["tab"] == child
    });
    e.request("shutdown", json!({})).unwrap();
}

#[test]
fn select_popups_cursors_and_the_context_menu() {
    let page = "<html><head><title>widgets</title></head><body style='margin:0'>\
        <select id=s style='position:absolute;left:10px;top:10px;width:150px;font-size:16px'>\
        <option>one</option><option>two</option><option>three</option><option>four</option></select>\
        <div style='position:absolute;left:0;top:100px;width:400px;height:100px;cursor:pointer'></div>\
        <div style='position:absolute;left:0;top:200px;width:400px;height:100px;cursor:text'></div>\
        <a href='http://example.invalid/linked' style='position:absolute;left:250px;top:10px'>a link</a>\
        </body></html>";
    let Some((mut e, _)) = spawn_engine() else {
        return;
    };
    let tab = e.open(&data_url(page), 400, 300);
    e.title_is(&tab, "widgets");
    // The cursor follows the element under the pointer.
    e.painted(&tab);
    for (y, cursor) in [(150, "pointer"), (250, "text")] {
        e.notify(
            "tab/input",
            json!({"tab": tab, "event": {"type": "mouseMove", "x": 200, "y": y}}),
        );
        let c = cursor.to_owned();
        e.wait(&format!("the {cursor} cursor"), |m| {
            m["method"] == "tab/cursor" && m["params"]["cursor"] == c
        });
    }
    // A click on the <select> opens its list: frames flagged with the popup's rectangle.
    e.click(&tab, 60, 20, "left");
    let f = e.wait("a frame with the popup", |m| {
        m["method"] == "tab/frame"
            && m["params"]["tab"] == tab
            && m["params"].get("popup").is_some()
    });
    let popup = &f["params"]["popup"];
    assert!(popup["height"].as_i64().unwrap() > 40, "{popup}");
    assert!(
        popup["y"].as_i64().unwrap() >= 20,
        "below the select: {popup}"
    );
    let dirty = f["params"]["dirty"].as_array().unwrap();
    assert!(
        dirty.iter().any(|d| d == popup),
        "the popup is among the dirty rectangles"
    );
    // Keys choose in the list and Enter picks: the page sees the change.
    for code in [40, 13] {
        for t in ["rawKeyDown", "keyUp"] {
            e.notify(
                "tab/input",
                json!({"tab": tab, "event": {"type": t, "windowsKeyCode": code}}),
            );
        }
    }
    let deadline = Instant::now() + Duration::from_secs(10);
    while e.evaluate(&tab, "document.getElementById('s').value") != "two" {
        assert!(
            Instant::now() < deadline,
            "the list did not pick the second option"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    // A right click asks the shell for its menu, with the link under the pointer.
    e.click(&tab, 270, 18, "right");
    let menu = e.wait("tab/contextMenu", |m| m["method"] == "tab/contextMenu");
    assert_eq!(menu["params"]["linkUrl"], "http://example.invalid/linked");
    assert_eq!(menu["params"]["editable"], false);
    assert_eq!(menu["params"]["tab"], json!(tab));
    e.request("shutdown", json!({})).unwrap();
}

#[test]
fn javascript_dialogs_and_file_choosers_round_trip_through_the_shell() {
    let page = button_page(
        "dialogs",
        "alert('hello'); const ok = confirm('sure?'); const name = prompt('name?', 'def'); \
         document.title = 'answered ' + ok + ' ' + name;",
    );
    let Some((mut e, _)) = spawn_engine() else {
        return;
    };
    let tab = e.open(&data_url(&page), 400, 300);
    e.title_is(&tab, "dialogs");
    e.click(&tab, 200, 150, "left");
    let mut answers = Vec::new();
    for (kind, message, accept, text) in [
        ("alert", "hello", true, None),
        ("confirm", "sure?", true, None),
        ("prompt", "name?", true, Some("Ada")),
    ] {
        let d = e.wait(&format!("the {kind}"), |m| m["method"] == "tab/dialog");
        let p = &d["params"];
        assert_eq!(
            (p["kind"].as_str(), p["message"].as_str()),
            (Some(kind), Some(message)),
            "{d}"
        );
        if kind == "prompt" {
            assert_eq!(p["defaultText"], "def");
        }
        let id = p["id"].as_u64().unwrap();
        let mut answer = json!({"tab": tab, "id": id, "accept": accept});
        if let Some(t) = text {
            answer["text"] = json!(t);
        }
        e.notify("tab/dialogAnswer", answer);
        let closed = e.wait("tab/dialogClosed", |m| {
            m["method"] == "tab/dialogClosed" && m["params"]["id"] == id
        });
        assert_eq!(closed["params"]["accepted"], accept);
        answers.push(id);
    }
    e.title_is(&tab, "answered true Ada");
    // A file chooser: the shell's answer reaches the page as a chosen file.
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("chosen.txt");
    std::fs::write(&file, b"twelve bytes").unwrap();
    let page = "<html><head><title>files</title></head><body style='margin:0'>\
        <input type=file id=f accept='.txt' style='width:100vw;height:100vh' \
        onchange=\"document.title = this.files[0].name + ' ' + this.files[0].size\"></body></html>";
    e.request("tab/navigate", json!({"tab": tab, "url": data_url(page)}))
        .unwrap();
    e.title_is(&tab, "files");
    e.click(&tab, 200, 150, "left");
    let d = e.wait("the file chooser", |m| m["method"] == "tab/dialog");
    assert_eq!(d["params"]["kind"], "file");
    assert_eq!(d["params"]["mode"], "open");
    assert_eq!(d["params"]["accept"], json!([".txt"]));
    e.notify(
        "tab/dialogAnswer",
        json!({"tab": tab, "id": d["params"]["id"], "accept": true, "files": [file.to_string_lossy()]}),
    );
    e.title_is(&tab, "chosen.txt 12");
    e.request("shutdown", json!({})).unwrap();
}

#[test]
fn permissions_authentication_and_downloads() {
    let s = site(&[
        (
            "/",
            "text/html",
            button_page(
                "perm",
                "navigator.geolocation.getCurrentPosition(() => document.title = 'located', \
                 e => document.title = 'denied ' + e.code)",
            )
            .as_bytes(),
        ),
        ("/file", "application/octet-stream", b"0123456789abcdef"),
        (
            "/big",
            "application/octet-stream",
            "x".repeat(4096).as_bytes(),
        ),
        (
            "/auth",
            "text/html",
            b"<html><head><title>signed in</title></head></html>",
        ),
    ]);
    let downloads = tempfile::tempdir().unwrap();
    let Some((mut e, _)) = spawn_engine_with(
        &[],
        json!({"downloadDir": downloads.path(), "maxDownloadBytes": 1000}),
    ) else {
        return;
    };
    let tab = e.open(&s.url("/"), 400, 300);
    e.title_is(&tab, "perm");
    // A permission prompt: denied by the shell, the page hears so.
    e.click(&tab, 200, 150, "left");
    let p = e.wait("tab/permission", |m| m["method"] == "tab/permission");
    assert_eq!(p["params"]["permissions"], json!(["geolocation"]));
    assert_eq!(
        p["params"]["origin"]
            .as_str()
            .unwrap()
            .trim_end_matches('/'),
        s.base
    );
    e.notify(
        "tab/permissionAnswer",
        json!({"tab": tab, "id": p["params"]["id"], "allow": false}),
    );
    e.title_is(&tab, "denied 1");
    // Downloads land in the folder, without a prompt; over the limit they are refused.
    e.notify(
        "tab/action",
        json!({"tab": tab, "action": "download", "url": s.url("/file")}),
    );
    let done = e.wait("the download", |m| {
        m["method"] == "tab/download" && m["params"]["state"] == "complete"
    });
    let path = std::path::PathBuf::from(done["params"]["path"].as_str().unwrap());
    assert_eq!(path.parent(), Some(downloads.path()));
    assert_eq!(path.file_name().unwrap(), "report.bin");
    assert_eq!(std::fs::read(&path).unwrap(), b"0123456789abcdef");
    e.notify(
        "tab/action",
        json!({"tab": tab, "action": "download", "url": s.url("/big")}),
    );
    let refused = e.wait("the refusal", |m| {
        m["method"] == "tab/download" && m["params"]["state"] == "refused"
    });
    assert!(
        refused["params"]["message"]
            .as_str()
            .unwrap()
            .contains("limit"),
        "{refused}"
    );
    // An authentication challenge: the shell's credentials sign in.
    e.request("tab/navigate", json!({"tab": tab, "url": s.url("/auth")}))
        .unwrap();
    let a = e.wait("the auth dialog", |m| {
        m["method"] == "tab/dialog" && m["params"]["kind"] == "auth"
    });
    assert_eq!(a["params"]["realm"], "eludite-test");
    e.notify(
        "tab/dialogAnswer",
        json!({"tab": tab, "id": a["params"]["id"], "accept": true, "username": "u", "password": "p"}),
    );
    e.title_is(&tab, "signed in");
    e.request("shutdown", json!({})).unwrap();
}

#[test]
fn devtools_opens_as_a_tab_and_closes_with_its_page() {
    let Some((mut e, _)) = spawn_engine() else {
        return;
    };
    let tab = e.open(&data_url(QUADRANTS), 400, 300);
    e.title_is(&tab, "quadrants");
    let r = e
        .request(
            "tab/devtools",
            json!({"tab": tab, "width": 800, "height": 500}),
        )
        .unwrap();
    assert_eq!(r["created"], true);
    let devtools = r["devtools"].as_str().unwrap().to_owned();
    assert!(
        e.regions.len() >= 2,
        "the DevTools tab's region came before the answer"
    );
    let f = e.wait("a DevTools frame", |m| {
        m["method"] == "tab/frame" && m["params"]["tab"] == devtools
    });
    assert_eq!(
        (
            f["params"]["width"].as_u64(),
            f["params"]["height"].as_u64()
        ),
        (Some(800), Some(500))
    );
    // The front end is connected to the page: its Elements panel reads the page's DOM, so its title names the page.
    let dt = devtools.clone();
    let title = e.wait("the DevTools title", |m| {
        m["method"] == "tab/state"
            && m["params"]["tab"] == dt
            && m["params"]["title"]
                .as_str()
                .is_some_and(|t| t.starts_with("DevTools - "))
    });
    eprintln!("DevTools tab title: {}", title["params"]["title"]);
    // The page's own CDP session still answers the shell (the front end's ids are kept apart).
    assert_eq!(e.evaluate(&tab, "document.title"), "quadrants");
    // Asked again: the same tab. It is not a page target.
    let again = e
        .request(
            "tab/devtools",
            json!({"tab": tab, "width": 800, "height": 500}),
        )
        .unwrap();
    assert_eq!(
        (again["created"].as_bool(), again["devtools"].as_str()),
        (Some(false), Some(devtools.as_str()))
    );
    e.notify(
        "tab/cdp",
        json!({"tab": devtools, "message": {"id": 3, "method": "Runtime.evaluate", "params": {}}}),
    );
    let refused = e.wait("the refusal", |m| {
        m["method"] == "tab/cdpEvent" && m["params"]["message"]["id"] == 3
    });
    assert!(refused["params"]["message"]["error"].is_object());
    // Closing the page closes its DevTools.
    e.request("tab/close", json!({"tab": tab})).unwrap();
    let mut gone = std::collections::BTreeSet::new();
    while gone.len() < 2 {
        let c = e.wait("tab/closed", |m| m["method"] == "tab/closed");
        gone.insert(c["params"]["tab"].as_str().unwrap().to_owned());
    }
    assert!(gone.contains(&tab) && gone.contains(&devtools), "{gone:?}");
    e.request("shutdown", json!({})).unwrap();
}

#[test]
fn ime_composition_commits_text_and_state_carries_history_and_favicon() {
    let svg = b"<svg xmlns='http://www.w3.org/2000/svg' width='16' height='16'><rect width='16' height='16' fill='red'/></svg>";
    let page = "<html><head><title>ime</title><link rel=icon href='/icon.svg'></head>\
         <body style='margin:0'><input id=i autofocus style='width:300px;font-size:20px'></body></html>";
    let s = site(&[
        ("/", "text/html", page.as_bytes()),
        ("/icon.svg", "image/svg+xml", svg),
    ]);
    let Some((mut e, _)) = spawn_engine() else {
        return;
    };
    let tab = e.open(&s.url("/"), 400, 300);
    e.title_is(&tab, "ime");
    let t = tab.clone();
    let fav = e.wait("the favicon", |m| {
        m["method"] == "tab/state"
            && m["params"]["tab"] == t
            && m["params"]["favicon"]
                .as_str()
                .is_some_and(|f| f.starts_with("data:image/png;base64,"))
    });
    assert!(fav["params"]["favicon"].as_str().unwrap().len() > 30);
    e.click(&tab, 100, 15, "left");
    e.notify("tab/input", json!({"tab": tab, "event": {"type": "imeSetComposition", "text": "かな", "selectionStart": 2, "selectionEnd": 2}}));
    let deadline = Instant::now() + Duration::from_secs(10);
    while e.evaluate(&tab, "document.getElementById('i').value") != "かな" {
        assert!(Instant::now() < deadline, "the composition did not show");
        std::thread::sleep(Duration::from_millis(50));
    }
    e.notify(
        "tab/input",
        json!({"tab": tab, "event": {"type": "imeCommitText", "text": "仮名"}}),
    );
    let deadline = Instant::now() + Duration::from_secs(10);
    while e.evaluate(&tab, "document.getElementById('i').value") != "仮名" {
        assert!(
            Instant::now() < deadline,
            "the commit did not replace the composition"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    // History: a second page can go back.
    e.request(
        "tab/navigate",
        json!({"tab": tab, "url": data_url(QUADRANTS)}),
    )
    .unwrap();
    let t = tab.clone();
    e.wait("canGoBack", |m| {
        m["method"] == "tab/state"
            && m["params"]["tab"] == t
            && m["params"]["canGoBack"] == true
            && m["params"]["loading"] == false
    });
    e.request("shutdown", json!({})).unwrap();
}
