//! Brief 0004's end-to-end proof: attach to a real .NET Framework 4.8 process, break on the line that updates
//! `counter`, and read it, with the DAP client on TCP.
//!
//! `cargo test -p eludite-dbg-netfx --features e2e -- --ignored`
//!
//! - `loopback`: the brief's proving test (Windows): adapter on `127.0.0.1:0`.
//! - `non_loopback_address`: the same session with the adapter bound to this machine's non-loopback IPv4 address
//!   (or `ELUDITE_NETFX_HOST`) and the client connecting to that address (Windows).
//! - `remote_client`: the client half alone, for a second machine (any OS): set `ELUDITE_NETFX_REMOTE=host:port`
//!   (an adapter started with `--listen 0.0.0.0:PORT` on the Windows box), `ELUDITE_NETFX_REMOTE_PID` (the Counter
//!   fixture's pid there) and optionally `ELUDITE_NETFX_REMOTE_LINE` (default: the marked line of this checkout's
//!   `Program.cs`). Without `ELUDITE_NETFX_REMOTE` it reports that it was skipped.

#![cfg(feature = "e2e")]

use std::io::{BufReader, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

use eludite_dbg_netfx::framing::{read_message, write_message};
use serde_json::{Value, json};

const MARKER: &str = "// BREAKPOINT";

fn program_cs() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/Counter/Program.cs")
}

fn breakpoint_line() -> u32 {
    let text = std::fs::read_to_string(program_cs()).unwrap();
    text.lines()
        .position(|l| l.contains(MARKER))
        .map(|i| i as u32 + 1)
        .expect("Program.cs has the BREAKPOINT marker")
}

/// A minimal DAP client: a reader thread turns frames into JSON values.
struct Dap {
    stream: TcpStream,
    rx: Receiver<Value>,
    seq: i64,
    events: Vec<Value>,
}

impl Dap {
    fn connect(addr: SocketAddr) -> Self {
        let stream = TcpStream::connect_timeout(&addr, Duration::from_secs(5)).unwrap();
        stream.set_nodelay(true).unwrap();
        let mut reader = BufReader::new(stream.try_clone().unwrap());
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            while let Ok(Some(body)) = read_message(&mut reader) {
                let v: Value = serde_json::from_slice(&body).unwrap();
                if tx.send(v).is_err() {
                    break;
                }
            }
        });
        Self {
            stream,
            rx,
            seq: 0,
            events: Vec::new(),
        }
    }

    fn send(&mut self, command: &str, arguments: Value) -> i64 {
        self.seq += 1;
        let m = json!({ "seq": self.seq, "type": "request", "command": command, "arguments": arguments });
        write_message(&mut self.stream, &serde_json::to_vec(&m).unwrap()).unwrap();
        self.seq
    }

    /// Sends a request and waits for its response; events that arrive meanwhile are kept.
    fn request(&mut self, command: &str, arguments: Value) -> Value {
        let seq = self.send(command, arguments);
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            let m = self
                .rx
                .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                .unwrap_or_else(|_| panic!("no response to {command}"));
            if m["type"] == "response" && m["request_seq"] == seq {
                assert_eq!(m["success"], true, "{command} failed: {m}");
                return m;
            }
            self.events.push(m);
        }
    }

    fn wait_event(&mut self, name: &str, timeout: Duration) -> Value {
        if let Some(i) = self.events.iter().position(|e| e["event"] == name) {
            return self.events.remove(i);
        }
        let deadline = Instant::now() + timeout;
        loop {
            let m = self
                .rx
                .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                .unwrap_or_else(|_| {
                    panic!("no {name} event within {timeout:?}; saw {:?}", self.events)
                });
            if m["type"] == "event" && m["event"] == name {
                return m;
            }
            self.events.push(m);
        }
    }
}

/// What one session measured.
#[derive(Debug)]
struct Measured {
    attach_to_initialized: Duration,
    first_counter: i32,
    second_counter: i32,
    /// The client's clock reading when each `stopped` event arrived (the fixture's clock; Windows QPC).
    stopped_at: Vec<Option<i64>>,
    /// The adapter's working set at the first stop, in bytes.
    paused_working_set: Option<u64>,
}

/// The brief's DAP sequence against an attached-to Counter process. `rss` samples the adapter's memory while paused.
fn run_session(
    addr: SocketAddr,
    pid: u32,
    source: &str,
    line: u32,
    mut rss: impl FnMut() -> Option<u64>,
    clock: &dyn Fn() -> Option<i64>,
) -> Measured {
    let mut stopped_at = Vec::new();
    let mut dap = Dap::connect(addr);
    let init = dap.request(
        "initialize",
        json!({ "clientID": "eludite-e2e", "adapterID": "netfx", "linesStartAt1": true, "columnsStartAt1": true, "pathFormat": "path" }),
    );
    assert_eq!(init["body"]["supportsConfigurationDoneRequest"], true);

    let t = Instant::now();
    dap.request("attach", json!({ "processId": pid }));
    dap.wait_event("initialized", Duration::from_secs(10));
    let attach_to_initialized = t.elapsed();

    let bps = dap.request(
        "setBreakpoints",
        json!({ "source": { "path": source }, "breakpoints": [{ "line": line }] }),
    );
    let bp = &bps["body"]["breakpoints"][0];
    eprintln!("setBreakpoints: {bp}");
    dap.request("configurationDone", json!({}));

    let stopped = dap.wait_event("stopped", Duration::from_secs(20));
    stopped_at.push(clock());
    assert_eq!(stopped["body"]["reason"], "breakpoint", "{stopped}");
    let thread_id = stopped["body"]["threadId"].as_u64().unwrap();

    let threads = dap.request("threads", json!({}));
    assert!(
        threads["body"]["threads"]
            .as_array()
            .unwrap()
            .iter()
            .any(|t| t["id"] == thread_id),
        "{threads}"
    );

    let first_counter = inspect(&mut dap, thread_id, line);
    let paused_working_set = rss();
    if let Some(bytes) = paused_working_set {
        let mb = bytes as f64 / (1024.0 * 1024.0);
        eprintln!("adapter working set while paused: {mb:.1} MB");
        assert!(
            mb < 100.0,
            "adapter working set {mb:.1} MB exceeds the 100 MB budget"
        );
    }

    dap.request("continue", json!({ "threadId": thread_id }));
    let stopped = dap.wait_event("stopped", Duration::from_secs(20));
    stopped_at.push(clock());
    assert_eq!(stopped["body"]["reason"], "breakpoint");
    let second_counter = inspect(
        &mut dap,
        stopped["body"]["threadId"].as_u64().unwrap(),
        line,
    );
    assert!(
        second_counter > first_counter,
        "counter went from {first_counter} to {second_counter}"
    );

    dap.request("disconnect", json!({ "terminateDebuggee": false }));
    dap.wait_event("terminated", Duration::from_secs(5));
    Measured {
        attach_to_initialized,
        first_counter,
        second_counter,
        stopped_at,
        paused_working_set,
    }
}

/// stackTrace, scopes and variables at the breakpoint; returns `counter`.
fn inspect(dap: &mut Dap, thread_id: u64, line: u32) -> i32 {
    let st = dap.request("stackTrace", json!({ "threadId": thread_id, "levels": 20 }));
    let top = &st["body"]["stackFrames"][0];
    eprintln!("top frame: {top}");
    assert!(
        top["name"].as_str().unwrap().ends_with("Program.Tick"),
        "top frame is {top}"
    );
    assert_eq!(top["line"], line, "{top}");
    assert!(
        top["source"]["path"]
            .as_str()
            .unwrap()
            .ends_with("Program.cs"),
        "{top}"
    );
    let caller = &st["body"]["stackFrames"][1];
    assert!(
        caller["name"].as_str().unwrap().ends_with("Program.Main"),
        "caller is {caller}"
    );

    let scopes = dap.request("scopes", json!({ "frameId": top["id"] }));
    let locals = &scopes["body"]["scopes"][0];
    assert_eq!(locals["name"], "Locals");
    let vars = dap.request(
        "variables",
        json!({ "variablesReference": locals["variablesReference"] }),
    );
    eprintln!("locals: {}", vars["body"]["variables"]);
    let counter = vars["body"]["variables"]
        .as_array()
        .unwrap()
        .iter()
        .find(|v| v["name"] == "counter")
        .unwrap_or_else(|| panic!("no counter in {vars}"));
    assert_eq!(counter["type"], "int", "{counter}");
    let value: i32 = counter["value"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap_or_else(|_| panic!("counter is not an integer: {counter}"));
    // `counter = iteration * 2` has run; `counter + 1` has not.
    assert_eq!(
        value % 2,
        0,
        "counter {value} should be even at the breakpoint"
    );
    value
}

#[test]
#[ignore = "needs a real ELUDITE_NETFX_REMOTE adapter; see the file docs"]
fn remote_client() {
    let Ok(remote) = std::env::var("ELUDITE_NETFX_REMOTE") else {
        eprintln!("remote_client skipped: ELUDITE_NETFX_REMOTE is not set");
        return;
    };
    use std::net::ToSocketAddrs;
    let addr = remote
        .to_socket_addrs()
        .unwrap()
        .next()
        .expect("ELUDITE_NETFX_REMOTE resolves");
    let pid: u32 = std::env::var("ELUDITE_NETFX_REMOTE_PID")
        .expect("ELUDITE_NETFX_REMOTE_PID")
        .parse()
        .unwrap();
    let line = std::env::var("ELUDITE_NETFX_REMOTE_LINE")
        .ok()
        .map(|l| l.parse().unwrap())
        .unwrap_or_else(breakpoint_line);
    // The client's own path: the adapter maps it to the PDB's document by file name (no shared paths).
    let source = program_cs().display().to_string();
    let m = run_session(addr, pid, &source, line, || None, &|| None);
    eprintln!(
        "RESULT remote_client against {remote}: attach->initialized {:.1} ms; counter {} then {}; \
         stopped events {} (no shared clock, so no hit latency); adapter memory not sampled ({:?})",
        m.attach_to_initialized.as_secs_f64() * 1000.0,
        m.first_counter,
        m.second_counter,
        m.stopped_at.len(),
        m.paused_working_set,
    );
}

#[cfg(windows)]
mod windows_only {
    use super::*;
    use std::collections::HashMap;
    use std::io::{BufRead, Read};
    use std::process::{Child, Command, Stdio};
    use std::sync::{Arc, Mutex, OnceLock};

    /// The tests share one fixture build and run one at a time (they would race on the build and on timing).
    static SERIAL: Mutex<()> = Mutex::new(());
    static FIXTURE: OnceLock<PathBuf> = OnceLock::new();

    /// iteration -> the fixture's QueryPerformanceCounter just before it called Tick.
    type Ticks = Arc<Mutex<HashMap<i32, i64>>>;

    fn qpc() -> Option<i64> {
        let mut v = 0i64;
        #[allow(unsafe_code)]
        // SAFETY: writes one i64.
        unsafe { windows::Win32::System::Performance::QueryPerformanceCounter(&mut v) }.ok()?;
        Some(v)
    }

    fn qpc_frequency() -> i64 {
        let mut v = 0i64;
        #[allow(unsafe_code)]
        // SAFETY: writes one i64.
        unsafe { windows::Win32::System::Performance::QueryPerformanceFrequency(&mut v) }.unwrap();
        v
    }

    struct Killer(Child);
    impl Drop for Killer {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }

    fn build_fixture() -> PathBuf {
        let artifacts = Path::new(env!("CARGO_TARGET_TMPDIR")).join("netfx-fixture");
        let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/build.ps1");
        let out = Command::new("powershell")
            .args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-File"])
            .arg(&script)
            .arg("-ArtifactsPath")
            .arg(&artifacts)
            .output()
            .expect("run powershell");
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(
            out.status.success(),
            "fixture build failed:\n{stdout}\n{}",
            String::from_utf8_lossy(&out.stderr)
        );
        PathBuf::from(stdout.lines().last().unwrap().trim())
    }

    fn start_fixture(exe: &Path) -> (Killer, u32, Ticks) {
        let mut child = Command::new(exe)
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("start Counter.exe");
        let mut line = String::new();
        let mut stdout = BufReader::new(child.stdout.take().unwrap());
        stdout.read_line(&mut line).unwrap();
        let ticks: Ticks = Arc::default();
        let sink = ticks.clone();
        std::thread::spawn(move || {
            for l in stdout.lines().map_while(Result::ok) {
                let mut parts = l.split(' ').skip(1);
                if let (Some(Ok(i)), Some(Ok(t))) = (
                    parts.next().map(str::parse::<i32>),
                    parts.next().map(str::parse::<i64>),
                ) {
                    sink.lock().unwrap().insert(i, t);
                }
            }
        });
        let pid: u32 = line
            .trim()
            .strip_prefix("ready ")
            .unwrap_or_else(|| panic!("fixture said {line:?}"))
            .parse()
            .unwrap();
        assert_eq!(pid, child.id());
        (Killer(child), pid, ticks)
    }

    /// Starts the adapter; returns it, its address, and a channel of its stderr lines.
    fn start_adapter(listen: &str) -> (Killer, SocketAddr, Receiver<String>) {
        let mut child = Command::new(env!("CARGO_BIN_EXE_eludite-dbg-netfx"))
            .args(["--listen", listen])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("start the adapter");
        let stderr = BufReader::new(child.stderr.take().unwrap());
        let mut stdout = child.stdout.take().unwrap();
        // Invariant 10 in spirit: nothing on stdout.
        std::thread::spawn(move || {
            let mut s = String::new();
            let _ = stdout.read_to_string(&mut s);
            assert!(s.is_empty(), "the adapter wrote to stdout: {s:?}");
        });
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            for line in stderr.lines().map_while(Result::ok) {
                eprintln!("[adapter] {line}");
                if tx.send(line).is_err() {
                    break;
                }
            }
        });
        let first = rx
            .recv_timeout(Duration::from_secs(10))
            .expect("adapter printed its address");
        let addr: SocketAddr = first
            .rsplit("listening on ")
            .next()
            .unwrap()
            .trim()
            .parse()
            .unwrap_or_else(|_| panic!("unexpected first line {first:?}"));
        (Killer(child), addr, rx)
    }

    fn working_set(pid: u32) -> Option<u64> {
        use windows::Win32::Foundation::CloseHandle;
        use windows::Win32::System::ProcessStatus::{
            GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS,
        };
        use windows::Win32::System::Threading::{OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};
        #[allow(unsafe_code)]
        // SAFETY: Win32 calls with a valid, owned handle and a correctly sized buffer.
        unsafe {
            let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
            let mut c = PROCESS_MEMORY_COUNTERS {
                cb: std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32,
                ..Default::default()
            };
            let ok = GetProcessMemoryInfo(h, &mut c, c.cb).is_ok();
            let _ = CloseHandle(h);
            ok.then_some(c.WorkingSetSize as u64)
        }
    }

    /// Pulls a number out of the adapter's log lines: `"<prefix> <n> ms"`.
    fn logged_ms(lines: &[String], prefix: &str) -> Vec<f64> {
        lines
            .iter()
            .filter_map(|l| l.split(prefix).nth(1))
            .filter_map(|rest| rest.trim().split(' ').next()?.parse().ok())
            .collect()
    }

    fn full_run(listen: &str, connect_host: Option<&str>) {
        let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        let exe = FIXTURE.get_or_init(build_fixture);
        let (_fixture, pid, ticks) = start_fixture(exe);
        let (mut adapter, addr, log) = start_adapter(listen);
        let addr = match connect_host {
            Some(h) => SocketAddr::new(h.parse().unwrap(), addr.port()),
            None => addr,
        };
        let adapter_pid = adapter.0.id();
        let source = program_cs().display().to_string();
        let line = breakpoint_line();
        let m = run_session(addr, pid, &source, line, || working_set(adapter_pid), &qpc);

        // Breakpoint hit to `stopped`, end to end: from the fixture's clock reading just before it called Tick to
        // the client's reading when `stopped` arrived (one machine, one QueryPerformanceCounter).
        std::thread::sleep(Duration::from_millis(100));
        let freq = qpc_frequency() as f64;
        let ticks = ticks.lock().unwrap().clone();
        let end_to_end: Vec<f64> = [m.first_counter, m.second_counter]
            .iter()
            .zip(&m.stopped_at)
            .map(|(&counter, &at)| {
                let before = ticks[&(counter / 2)];
                (at.unwrap() - before) as f64 * 1000.0 / freq
            })
            .collect();

        // The adapter exits after the client disconnects; the debuggee keeps running (detach, not kill).
        let deadline = Instant::now() + Duration::from_secs(10);
        while adapter.0.try_wait().unwrap().is_none() {
            assert!(
                Instant::now() < deadline,
                "adapter did not exit after disconnect"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(
            adapter.0.try_wait().unwrap().unwrap().success(),
            "adapter exit status"
        );
        let lines: Vec<String> = log.try_iter().collect();
        let hits = logged_ms(&lines, "callback to stopped event");
        let init = logged_ms(&lines, "attach request to initialized event");
        eprintln!(
            "RESULT listen={listen} connect={addr} attach->initialized (client) {:.1} ms, (adapter) {init:?} ms; \
             breakpoint hit->stopped at the client {end_to_end:.2?} ms, callback->stopped in the adapter {hits:?} ms; \
             adapter working set while paused {:.1} MB; counter {} then {}",
            m.attach_to_initialized.as_secs_f64() * 1000.0,
            m.paused_working_set.unwrap_or(0) as f64 / (1024.0 * 1024.0),
            m.first_counter,
            m.second_counter
        );
        assert!(
            m.attach_to_initialized < Duration::from_secs(2),
            "attach to initialized took {:?}",
            m.attach_to_initialized
        );
        assert_eq!(hits.len(), 2, "two breakpoint hits logged: {lines:?}");
        for h in end_to_end {
            assert!(h < 200.0, "breakpoint hit to stopped took {h} ms");
        }
        let mut fixture = _fixture;
        assert!(
            fixture.0.try_wait().unwrap().is_none(),
            "the debuggee died after detach"
        );
    }

    #[test]
    #[ignore = "Windows end-to-end against .NET Framework 4.8; run with --features e2e -- --ignored"]
    fn loopback() {
        full_run("127.0.0.1:0", None);
    }

    /// This machine's outbound IPv4 address (no packet is sent: a UDP connect only picks a route).
    fn local_ipv4() -> Option<String> {
        if let Ok(h) = std::env::var("ELUDITE_NETFX_HOST") {
            return Some(h);
        }
        let s = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
        s.connect("192.0.2.1:9").ok()?;
        let ip = s.local_addr().ok()?.ip();
        (!ip.is_loopback() && !ip.is_unspecified()).then(|| ip.to_string())
    }

    #[test]
    #[ignore = "Windows end-to-end over a non-loopback address; run with --features e2e -- --ignored"]
    fn non_loopback_address() {
        let Some(ip) = local_ipv4() else {
            eprintln!("non_loopback_address skipped: no non-loopback IPv4 address");
            return;
        };
        let _ = std::io::stderr().flush();
        full_run(&format!("{ip}:0"), Some(&ip));
    }
}
