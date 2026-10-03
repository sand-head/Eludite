//! The control protocol against the real engine (brief 0031): create a tab, navigate to data URLs, receive frames
//! with the expected pixels at known points through the shared-memory ring, resize, a CDP round trip, mouse and key
//! input, close and shutdown. Needs the `cef` feature (CEF fetched by tools/cef/fetch.sh and CEF_PATH set); skips
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
    if !cfg!(feature = "cef") {
        eprintln!(
            "skipped: eludite-chromium was built without the cef feature (tools/cef/fetch.sh, then CEF_PATH=... \
             cargo test -p eludite-chromium --features cef)"
        );
        return None;
    }
    let exe = env!("CARGO_BIN_EXE_eludite-chromium");
    let profile = tempfile::tempdir().unwrap();
    let (ours, theirs) = shm::socket_pair().unwrap();
    let theirs_fd = theirs.as_raw_fd();
    let mut cmd = Command::new(exe);
    cmd.arg("--profile")
        .arg(profile.path())
        .args(["--frame-socket", "3"])
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
    let init = e.request(
        "initialize",
        json!({"clientName": "engine-test", "clientVersion": "0", "protocolVersion": 1}),
    );
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
