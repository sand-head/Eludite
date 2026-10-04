//! [`ExternalChrome`]: the [`Engine`] of brief 0023, a Chrome or Chromium process Eludite launches with the
//! workspace's own profile and drives over CDP on a websocket.
//!
//! The launch: `chrome --remote-debugging-port=0 --user-data-dir=<workspace>/.eludite/browser/profile --no-first-run
//! --no-default-browser-check --disable-background-networking --disable-sync --disable-default-apps
//! --window-size=W,H [--headless=new] [--no-sandbox] about:blank` ([`command_line`]). `--no-sandbox` only when
//! `ELUDITE_CHROME_NO_SANDBOX=1` is set (Chrome refuses to run as root without it; the shell never sets it). The
//! endpoint is read from Chrome's `DevTools listening on ws://...` line on stderr (10 s at most); stderr is drained
//! for the browser's life so Chrome never blocks on it, and its last lines go into launch errors. Then
//! `Target.setDiscoverTargets` and `Browser.getVersion`; tabs are `Target.createTarget` and `closeTarget`, sessions
//! `Target.attachToTarget` with `flatten: true`.
//!
//! When the connection closes without [`Engine::shutdown`] (Chrome crashed or was killed), the engine is gone:
//! pending requests fail, the log says so with the exit status, and the next [`Engine::launch`] starts a new
//! browser. `shutdown` asks `Browser.close`, then kills the process after 3 s.

use std::collections::{HashMap, VecDeque};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::LogSink;
use crate::connection::{CdpEvent, Connection, DEFAULT_TIMEOUT};
use crate::discovery::ChromeSearch;
use crate::engine::{DebugEndpoint, Engine, EngineConfig, EngineError, LaunchInfo, TargetInfo};

/// How long Chrome may take to print its DevTools endpoint.
pub const LAUNCH_TIMEOUT: Duration = Duration::from_secs(10);
/// How long `shutdown` waits for Chrome to exit after `Browser.close`.
const EXIT_TIMEOUT: Duration = Duration::from_secs(3);
/// Lines of Chrome's stderr kept for errors.
const STDERR_TAIL: usize = 30;

/// The environment variable that adds `--no-sandbox` (tests running as root).
pub const NO_SANDBOX_ENV: &str = "ELUDITE_CHROME_NO_SANDBOX";

/// The full command line: the executable, then its arguments.
pub fn command_line(exe: &Path, config: &EngineConfig, no_sandbox: bool) -> Vec<String> {
    let mut args = vec![
        exe.display().to_string(),
        "--remote-debugging-port=0".to_owned(),
        format!("--user-data-dir={}", config.profile_dir.display()),
        "--no-first-run".to_owned(),
        "--no-default-browser-check".to_owned(),
        "--disable-background-networking".to_owned(),
        "--disable-sync".to_owned(),
        "--disable-default-apps".to_owned(),
        format!("--window-size={},{}", config.viewport.0, config.viewport.1),
    ];
    if config.headless {
        args.push("--headless=new".to_owned());
    }
    if no_sandbox {
        args.push("--no-sandbox".to_owned());
    }
    args.push("about:blank".to_owned());
    args
}

/// Whether `ELUDITE_CHROME_NO_SANDBOX=1` is set.
pub fn no_sandbox_from_env() -> bool {
    std::env::var(NO_SANDBOX_ENV).is_ok_and(|v| v.trim() == "1")
}

/// The endpoint in a line of Chrome's stderr.
pub fn devtools_endpoint(line: &str) -> Option<&str> {
    let rest = line.split("DevTools listening on ").nth(1)?;
    let url = rest.split_whitespace().next()?;
    url.starts_with("ws://").then_some(url)
}

/// Create the profile directory, with a `.gitignore` of `*` in `.eludite/browser/` so the profile never shows up
/// in the workspace's source control whatever the workspace's own `.gitignore` says.
pub fn prepare_profile(profile: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(profile)?;
    if let Some(browser_dir) = profile.parent() {
        let ignore = browser_dir.join(".gitignore");
        if !ignore.exists() {
            std::fs::write(
                ignore,
                "# Eludite's browser profile for this workspace (cookies, storage, cache).\n*\n",
            )?;
        }
    }
    Ok(())
}

struct Running {
    conn: Connection,
    child: Arc<Mutex<Option<Child>>>,
    info: LaunchInfo,
    /// Session ids by target id.
    sessions: Arc<Mutex<HashMap<String, String>>>,
    stopping: Arc<AtomicBool>,
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// A Chrome or Chromium process driven over CDP.
pub struct ExternalChrome {
    config: EngineConfig,
    /// Where to look when `config.executable` is not set.
    search: ChromeSearch,
    /// `--no-sandbox`: from `ELUDITE_CHROME_NO_SANDBOX` unless a test says otherwise.
    no_sandbox: bool,
    log: LogSink,
    running: Option<Running>,
}

impl std::fmt::Debug for ExternalChrome {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ExternalChrome")
            .field("config", &self.config)
            .field("running", &self.is_running())
            .finish()
    }
}

impl ExternalChrome {
    /// An engine that launches nothing until [`Engine::launch`]. `search` is used when the configuration names no
    /// executable ([`ChromeSearch::defaults`] in the shell, [`ChromeSearch::from_env`] in tests).
    pub fn new(config: EngineConfig, search: ChromeSearch, log: LogSink) -> Self {
        Self {
            config,
            search,
            no_sandbox: no_sandbox_from_env(),
            log,
            running: None,
        }
    }

    /// Add `--no-sandbox` (or not) regardless of `ELUDITE_CHROME_NO_SANDBOX`: for tests that run as root, which
    /// cannot set the variable for themselves without `unsafe`. The shell never calls it.
    pub fn no_sandbox(mut self, on: bool) -> Self {
        self.no_sandbox = on;
        self
    }

    fn conn(&self) -> Result<&Running, EngineError> {
        match &self.running {
            Some(r) if !r.conn.is_closed() => Ok(r),
            _ => Err(EngineError::NotRunning),
        }
    }

    /// The browser's process id, while it runs.
    pub fn pid(&self) -> Option<u32> {
        let r = self.running.as_ref()?;
        lock(&r.child).as_ref().map(Child::id)
    }

    fn start(&mut self) -> Result<LaunchInfo, EngineError> {
        let search = ChromeSearch {
            configured: self.config.executable.clone(),
            ..self.search.clone()
        };
        let exe = search.find().map_err(EngineError::Launch)?;
        prepare_profile(&self.config.profile_dir).map_err(|e| {
            EngineError::Launch(format!(
                "cannot create the browser profile {}: {e}",
                self.config.profile_dir.display()
            ))
        })?;
        let args = command_line(&exe, &self.config, self.no_sandbox);
        let started = Instant::now();
        let mut child = Command::new(&exe)
            .args(&args[1..])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| EngineError::Launch(format!("cannot start {}: {e}", exe.display())))?;
        let stderr = child.stderr.take().expect("piped");
        let tail: Arc<Mutex<VecDeque<String>>> = Arc::default();
        let (tx, rx) = mpsc::channel();
        let drain_tail = tail.clone();
        std::thread::Builder::new()
            .name("chrome-stderr".into())
            .spawn(move || {
                let mut tx = Some(tx);
                for line in BufReader::new(stderr).lines() {
                    let Ok(line) = line else { break };
                    if let Some(url) = devtools_endpoint(&line)
                        && let Some(tx) = tx.take()
                    {
                        let _ = tx.send(url.to_owned());
                    }
                    let mut t = lock(&drain_tail);
                    if t.len() == STDERR_TAIL {
                        t.pop_front();
                    }
                    t.push_back(line);
                }
            })
            .map_err(|e| EngineError::Launch(e.to_string()))?;
        let fail = |child: &mut Child, why: String| {
            let _ = child.kill();
            let status = child.wait().ok().map(|s| s.to_string()).unwrap_or_default();
            let tail: Vec<String> = lock(&tail).iter().cloned().collect();
            EngineError::Launch(format!(
                "{why} ({}; {status}).{}",
                exe.display(),
                if tail.is_empty() {
                    String::new()
                } else {
                    format!(" Chrome said:\n{}", tail.join("\n"))
                }
            ))
        };
        let endpoint = match rx.recv_timeout(LAUNCH_TIMEOUT) {
            Ok(url) => url,
            Err(_) => {
                // Give the drain a moment to collect what Chrome printed before it exited.
                std::thread::sleep(Duration::from_millis(100));
                return Err(fail(
                    &mut child,
                    format!(
                        "Chrome did not report its DevTools endpoint within {} s",
                        LAUNCH_TIMEOUT.as_secs()
                    ),
                ));
            }
        };
        let conn = match Connection::connect(&endpoint, Duration::from_secs(5)) {
            Ok(c) => c,
            Err(e) => {
                return Err(fail(
                    &mut child,
                    format!("cannot connect to {endpoint}: {e}"),
                ));
            }
        };
        let version = conn
            .call(
                None,
                "Target.setDiscoverTargets",
                json!({"discover": true}),
                DEFAULT_TIMEOUT,
            )
            .and_then(|_| conn.call(None, "Browser.getVersion", Value::Null, DEFAULT_TIMEOUT));
        let version = match version {
            Ok(v) => v["product"].as_str().unwrap_or("Chrome").to_owned(),
            Err(e) => return Err(fail(&mut child, format!("the browser did not answer: {e}"))),
        };
        let info = LaunchInfo {
            executable: exe.display().to_string(),
            version,
            endpoint,
            ..LaunchInfo::default()
        };
        let child = Arc::new(Mutex::new(Some(child)));
        let stopping = Arc::new(AtomicBool::new(false));
        let sessions: Arc<Mutex<HashMap<String, String>>> = Arc::default();
        // Sessions of tabs closed from the browser's side end their subscriptions.
        let browser_events = conn.subscribe(None);
        let watcher_conn = conn.clone();
        let watcher_sessions = sessions.clone();
        let _ = std::thread::Builder::new()
            .name("cdp-targets".into())
            .spawn(move || {
                for e in browser_events {
                    if e.method == "Target.detachedFromTarget"
                        && let Some(s) = e.params["sessionId"].as_str()
                    {
                        watcher_conn.unsubscribe(Some(s));
                        lock(&watcher_sessions).retain(|_, v| v != s);
                    }
                }
            });
        let (hook_child, hook_stopping, hook_log) =
            (child.clone(), stopping.clone(), self.log.clone());
        conn.on_close(move || {
            if hook_stopping.load(Ordering::Acquire) {
                return;
            }
            // Chrome went away on its own: report how.
            let deadline = Instant::now() + Duration::from_secs(2);
            let mut status = None;
            while Instant::now() < deadline {
                match lock(&hook_child).as_mut().map(Child::try_wait) {
                    Some(Ok(Some(s))) => {
                        status = Some(s.to_string());
                        break;
                    }
                    Some(Ok(None)) => {}
                    _ => break,
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            if status.is_none()
                && let Some(c) = lock(&hook_child).as_mut()
            {
                // The connection broke but the process lingers: end it, the next command relaunches.
                let _ = c.kill();
                status = c.wait().ok().map(|s| s.to_string());
            }
            hook_log(&format!(
                "The browser exited unexpectedly ({}); the next browser command starts it again.",
                status.unwrap_or_else(|| "status unknown".into())
            ));
        });
        (self.log)(&format!(
            "Launched {} ({}) in {} ms; DevTools at {}; profile {}{}",
            info.executable,
            info.version,
            started.elapsed().as_millis(),
            info.endpoint,
            self.config.profile_dir.display(),
            if self.config.headless {
                "; headless"
            } else {
                ""
            }
        ));
        self.running = Some(Running {
            conn,
            child,
            info: info.clone(),
            sessions,
            stopping,
        });
        Ok(info)
    }

    fn stop(&mut self, say: bool) {
        let Some(r) = self.running.take() else { return };
        r.stopping.store(true, Ordering::Release);
        if !r.conn.is_closed() {
            let _ = r
                .conn
                .call(None, "Browser.close", Value::Null, Duration::from_secs(2));
        }
        let deadline = Instant::now() + EXIT_TIMEOUT;
        let mut guard = lock(&r.child);
        if let Some(c) = guard.as_mut() {
            loop {
                match c.try_wait() {
                    Ok(Some(_)) => break,
                    Ok(None) if Instant::now() < deadline => {
                        std::thread::sleep(Duration::from_millis(20))
                    }
                    _ => {
                        let _ = c.kill();
                        let _ = c.wait();
                        break;
                    }
                }
            }
        }
        guard.take();
        drop(guard);
        r.conn.close();
        if say {
            (self.log)("Closed the browser.");
        }
    }
}

impl Drop for ExternalChrome {
    fn drop(&mut self) {
        self.stop(false);
    }
}

impl Engine for ExternalChrome {
    fn name(&self) -> &'static str {
        "external-chrome"
    }

    fn configure(&mut self, config: EngineConfig) {
        self.config = config;
    }

    fn is_running(&self) -> bool {
        self.conn().is_ok()
    }

    fn launch(&mut self) -> Result<Option<LaunchInfo>, EngineError> {
        if self.is_running() {
            return Ok(None);
        }
        // A browser that went away leaves its process handle; reap it before starting another.
        self.stop(false);
        self.start().map(Some)
    }

    fn info(&self) -> Option<LaunchInfo> {
        self.conn().ok().map(|r| r.info.clone())
    }

    /// The `--remote-debugging-port=0` Chrome chose, from the endpoint it printed (brief 0038).
    fn debug_endpoint(&self) -> Option<DebugEndpoint> {
        crate::engine::endpoint_of(&self.info()?.endpoint)
    }

    fn targets(&self) -> Result<Vec<TargetInfo>, EngineError> {
        let r = self.conn()?;
        let v = r
            .conn
            .call(None, "Target.getTargets", json!({}), DEFAULT_TIMEOUT)?;
        Ok(v["targetInfos"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|t| {
                t["type"] == "page"
                    && !t["url"]
                        .as_str()
                        .unwrap_or_default()
                        .starts_with("devtools://")
            })
            .map(|t| TargetInfo {
                target_id: t["targetId"].as_str().unwrap_or_default().to_owned(),
                url: t["url"].as_str().unwrap_or_default().to_owned(),
                title: t["title"].as_str().unwrap_or_default().to_owned(),
            })
            .collect())
    }

    fn open_tab(&mut self, url: &str) -> Result<String, EngineError> {
        let r = self.conn()?;
        let v = r.conn.call(
            None,
            "Target.createTarget",
            json!({"url": url}),
            DEFAULT_TIMEOUT,
        )?;
        v["targetId"]
            .as_str()
            .map(str::to_owned)
            .ok_or_else(|| EngineError::Launch("Target.createTarget answered no targetId".into()))
    }

    fn close_tab(&mut self, target_id: &str) -> Result<(), EngineError> {
        let r = self.conn()?;
        r.conn.call(
            None,
            "Target.closeTarget",
            json!({"targetId": target_id}),
            DEFAULT_TIMEOUT,
        )?;
        if let Some(s) = lock(&r.sessions).remove(target_id) {
            r.conn.unsubscribe(Some(&s));
        }
        Ok(())
    }

    fn activate_tab(&mut self, target_id: &str) -> Result<(), EngineError> {
        let r = self.conn()?;
        r.conn.call(
            None,
            "Target.activateTarget",
            json!({"targetId": target_id}),
            DEFAULT_TIMEOUT,
        )?;
        Ok(())
    }

    fn attach(&mut self, target_id: &str) -> Result<String, EngineError> {
        let r = self.conn()?;
        if let Some(s) = lock(&r.sessions).get(target_id) {
            return Ok(s.clone());
        }
        let v = r.conn.call(
            None,
            "Target.attachToTarget",
            json!({"targetId": target_id, "flatten": true}),
            DEFAULT_TIMEOUT,
        )?;
        let s = v["sessionId"]
            .as_str()
            .ok_or_else(|| {
                EngineError::Launch("Target.attachToTarget answered no sessionId".into())
            })?
            .to_owned();
        lock(&r.sessions).insert(target_id.to_owned(), s.clone());
        Ok(s)
    }

    fn send(
        &self,
        session: &str,
        method: &str,
        params: Value,
        timeout: Duration,
    ) -> Result<Value, EngineError> {
        let r = self.conn()?;
        let session = (!session.is_empty()).then_some(session);
        Ok(r.conn.call(session, method, params, timeout)?)
    }

    fn send_many(
        &self,
        session: &str,
        calls: Vec<(String, Value)>,
        timeout: Duration,
    ) -> Vec<Result<Value, EngineError>> {
        match self.conn() {
            Ok(r) => r
                .conn
                .call_many((!session.is_empty()).then_some(session), calls, timeout)
                .into_iter()
                .map(|x| x.map_err(EngineError::from))
                .collect(),
            Err(e) => calls.iter().map(|_| Err(e.clone())).collect(),
        }
    }

    fn send_many_unless(
        &self,
        session: &str,
        calls: Vec<(String, Value)>,
        timeout: Duration,
        give_up: &dyn Fn() -> bool,
    ) -> Vec<Result<Value, EngineError>> {
        let r = match self.conn() {
            Ok(r) => r,
            Err(e) => return calls.iter().map(|_| Err(e.clone())).collect(),
        };
        let session = (!session.is_empty()).then_some(session);
        let sent = calls
            .into_iter()
            .map(|(m, p)| {
                let rx = r.conn.send(session, &m, p);
                (m, rx)
            })
            .collect();
        crate::engine::collect_unless(sent, timeout, give_up)
    }

    fn subscribe(&self, session: &str) -> Result<mpsc::Receiver<CdpEvent>, EngineError> {
        Ok(self.conn()?.conn.subscribe(Some(session)))
    }

    fn shutdown(&mut self) {
        let was = self.is_running();
        self.stop(was);
    }
}

/// The profile directory used when no workspace is open: `<cache>/eludite/browser/no-workspace/profile`.
pub fn fallback_profile() -> PathBuf {
    crate::discovery::default_cache_root()
        .and_then(|c| c.parent().map(Path::to_path_buf))
        .unwrap_or_else(std::env::temp_dir)
        .join("browser")
        .join("no-workspace")
        .join("profile")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_launch_command_line() {
        let config = EngineConfig {
            executable: None,
            profile_dir: PathBuf::from("/w/.eludite/browser/profile"),
            headless: false,
            viewport: (1280, 800),
            allow_no_sandbox: false,
        };
        let exe = Path::new("/opt/chrome/chrome");
        assert_eq!(
            command_line(exe, &config, false),
            [
                "/opt/chrome/chrome",
                "--remote-debugging-port=0",
                "--user-data-dir=/w/.eludite/browser/profile",
                "--no-first-run",
                "--no-default-browser-check",
                "--disable-background-networking",
                "--disable-sync",
                "--disable-default-apps",
                "--window-size=1280,800",
                "about:blank",
            ]
        );
        let headless = EngineConfig {
            headless: true,
            viewport: (390, 844),
            ..config
        };
        let line = command_line(exe, &headless, true);
        assert!(line.contains(&"--headless=new".to_owned()));
        assert!(line.contains(&"--no-sandbox".to_owned()));
        assert!(line.contains(&"--window-size=390,844".to_owned()));
        assert_eq!(line.last().unwrap(), "about:blank");
    }

    #[test]
    fn the_endpoint_line() {
        assert_eq!(
            devtools_endpoint(
                "DevTools listening on ws://127.0.0.1:41235/devtools/browser/3c1b-77"
            ),
            Some("ws://127.0.0.1:41235/devtools/browser/3c1b-77")
        );
        assert_eq!(devtools_endpoint("[1002/120000.1:ERROR:gpu] nothing"), None);
        assert_eq!(devtools_endpoint("DevTools listening on http://x"), None);
    }

    #[test]
    fn the_profile_is_ignored_by_git() {
        let t = tempfile::tempdir().unwrap();
        let profile = EngineConfig::profile_for(t.path());
        prepare_profile(&profile).unwrap();
        assert!(profile.is_dir());
        let ignore = std::fs::read_to_string(t.path().join(".eludite/browser/.gitignore")).unwrap();
        assert!(ignore.lines().any(|l| l == "*"));
        // A second launch keeps it.
        prepare_profile(&profile).unwrap();
    }

    #[test]
    fn a_missing_chrome_is_a_launch_error_naming_the_fetch_script() {
        let t = tempfile::tempdir().unwrap();
        let mut e = ExternalChrome::new(
            EngineConfig {
                executable: None,
                profile_dir: t.path().join("p"),
                headless: true,
                viewport: EngineConfig::VIEWPORT,
                allow_no_sandbox: false,
            },
            ChromeSearch::default(),
            Arc::new(|_: &str| {}),
        );
        match e.launch() {
            Err(EngineError::Launch(m)) => assert!(m.contains("tools/chrome/fetch.sh"), "{m}"),
            other => panic!("{other:?}"),
        }
        assert!(!e.is_running());
        assert_eq!(e.targets(), Err(EngineError::NotRunning));
    }
}
