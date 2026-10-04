//! The engine trait: what the browser commands need from a browser, so the external Chrome of brief 0023, the
//! embedded CEF engine of brief B and a later Servo are implementations, not redesigns (ADR-0008).
//!
//! The commands speak the Chrome DevTools Protocol to a tab's session ([`Engine::send`]); an engine without CDP
//! would translate. Tabs are the engine's page targets; the command layer gives them short ids (`t1`) and keeps
//! their state ([`crate::tab`]).

use std::path::PathBuf;
use std::sync::{Arc, mpsc};
use std::time::Duration;

use serde_json::Value;

use crate::connection::{CdpError, CdpEvent};
use crate::embedded::FrameSource;

/// What configures the next launch (the settings `browser.*` and the workspace).
#[derive(Debug, Clone, PartialEq)]
pub struct EngineConfig {
    /// `browser.chromePath` (or `ELUDITE_CHROME`); `None`: search ([`crate::discovery`]).
    pub executable: Option<PathBuf>,
    /// The profile: `<workspace>/.eludite/browser/profile`.
    pub profile_dir: PathBuf,
    /// `browser.headless`.
    pub headless: bool,
    /// `browser.viewport`: the window size.
    pub viewport: (u32, u32),
    /// The embedded engine may run without Chromium's sandbox where the sandbox cannot start (brief 0039): the
    /// workspace's opt-in `browser.allowNoSandbox`, or `ELUDITE_CHROME_NO_SANDBOX=1`. The engine then gets
    /// `--allow-no-sandbox`; it still runs sandboxed whenever it can.
    pub allow_no_sandbox: bool,
}

impl EngineConfig {
    /// The default viewport (`browser.viewport`'s default).
    pub const VIEWPORT: (u32, u32) = (1280, 800);

    /// The profile directory of a workspace folder.
    pub fn profile_for(workspace: &std::path::Path) -> PathBuf {
        workspace.join(".eludite").join("browser").join("profile")
    }
}

/// `1280x800` to `(1280, 800)`.
pub fn parse_viewport(s: &str) -> Option<(u32, u32)> {
    let (w, h) = s.trim().split_once(['x', 'X'])?;
    let w: u32 = w.trim().parse().ok()?;
    let h: u32 = h.trim().parse().ok()?;
    (w >= 10 && h >= 10 && w <= 20_000 && h <= 20_000).then_some((w, h))
}

/// What a launch started.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct LaunchInfo {
    pub executable: String,
    /// `Browser.getVersion`'s product (`HeadlessChrome/141.0.7390.37`).
    pub version: String,
    /// The browser's DevTools endpoint.
    pub endpoint: String,
    /// The embedded engine's sandbox: `namespaces`, `helper` or `none` (brief 0039).
    pub sandbox: Option<String>,
    /// The embedded engine's CEF folder.
    pub cef: Option<String>,
    /// How the embedded engine and its CEF were found (`setting`, `variable`, `beside`, `dev`; `beside`,
    /// `variable`, `cache`).
    pub found_by: Option<(String, String)>,
}

/// A dialog or prompt a tab's page waits on (brief 0032).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PendingDialog {
    /// The engine's id for it.
    pub id: u64,
    /// `alert`, `confirm`, `prompt`, `beforeunload`, `file`, `auth` or `permission`.
    pub kind: String,
    pub message: String,
    /// `prompt`: the default text.
    pub default_text: Option<String>,
}

/// How to answer a [`PendingDialog`].
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DialogAnswer {
    pub accept: bool,
    pub text: Option<String>,
    pub files: Vec<String>,
    pub username: Option<String>,
    pub password: Option<String>,
}

/// A tab's history and icon, when the engine knows them without asking the page.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TabHistory {
    pub can_go_back: bool,
    pub can_go_forward: bool,
    /// A data url, or empty.
    pub favicon: String,
}

/// Where a debugger reaches the browser's pages (brief 0038): its Chrome DevTools remote debugging endpoint.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DebugEndpoint {
    /// `127.0.0.1` for the engines Eludite starts.
    pub address: String,
    pub port: u16,
}

/// The host and port of a DevTools websocket url (`ws://127.0.0.1:41235/devtools/browser/...`).
pub fn endpoint_of(ws: &str) -> Option<DebugEndpoint> {
    let rest = ws.strip_prefix("ws://")?;
    let authority = rest.split('/').next()?;
    let (host, port) = authority.rsplit_once(':')?;
    Some(DebugEndpoint {
        address: host
            .trim_start_matches('[')
            .trim_end_matches(']')
            .to_owned(),
        port: port.parse().ok()?,
    })
}

/// A page target.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TargetInfo {
    pub target_id: String,
    pub url: String,
    pub title: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EngineError {
    /// No browser is running (it exited, or was never launched).
    NotRunning,
    /// Finding or starting the browser failed.
    Launch(String),
    /// A CDP request failed.
    Cdp(CdpError),
    /// [`Engine::send_many_unless`] gave up waiting.
    Interrupted,
}

impl std::fmt::Display for EngineError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EngineError::NotRunning => write!(f, "the browser is not running"),
            EngineError::Launch(m) => write!(f, "{m}"),
            EngineError::Cdp(e) => write!(f, "{e}"),
            EngineError::Interrupted => write!(f, "the call stopped waiting for the browser"),
        }
    }
}

impl std::error::Error for EngineError {}

impl From<CdpError> for EngineError {
    fn from(e: CdpError) -> Self {
        match e {
            CdpError::Closed => EngineError::NotRunning,
            e => EngineError::Cdp(e),
        }
    }
}

/// A request handed to the connection: the receiver of its answer, or why it was not sent.
pub type Sent = Result<mpsc::Receiver<Result<Value, CdpError>>, CdpError>;

/// The answers of requests sent at once, each waited for until `timeout` from now, giving up on the missing ones
/// once `give_up` says so (asked every 10 ms): [`Engine::send_many_unless`] of the engines that queue requests.
pub fn collect_unless(
    sent: Vec<(String, Sent)>,
    timeout: Duration,
    give_up: &dyn Fn() -> bool,
) -> Vec<Result<Value, EngineError>> {
    let deadline = std::time::Instant::now() + timeout;
    let mut gave_up = false;
    sent.into_iter()
        .map(|(m, r)| {
            let rx = r?;
            loop {
                if gave_up {
                    return Err(EngineError::Interrupted);
                }
                match rx.recv_timeout(Duration::from_millis(10)) {
                    Ok(r) => return r.map_err(EngineError::from),
                    Err(mpsc::RecvTimeoutError::Disconnected) => {
                        return Err(EngineError::NotRunning);
                    }
                    Err(mpsc::RecvTimeoutError::Timeout) => {
                        if std::time::Instant::now() >= deadline {
                            return Err(EngineError::Cdp(CdpError::Timeout {
                                method: m.clone(),
                                after: timeout,
                            }));
                        }
                        gave_up = give_up();
                    }
                }
            }
        })
        .collect()
}

/// A browser engine. Owned by one thread (the shell's `browser` worker); the events of a tab arrive on other
/// threads through [`Engine::subscribe`]'s channel.
pub trait Engine: Send {
    /// `external-chrome`.
    fn name(&self) -> &'static str;

    /// The configuration of the next launch. A running browser keeps the one it was launched with.
    fn configure(&mut self, config: EngineConfig);

    fn is_running(&self) -> bool;

    /// Start the browser unless it runs. `Some` when this call launched it.
    fn launch(&mut self) -> Result<Option<LaunchInfo>, EngineError>;

    /// What the running browser was launched as.
    fn info(&self) -> Option<LaunchInfo>;

    /// The page targets (tabs), in the browser's order.
    fn targets(&self) -> Result<Vec<TargetInfo>, EngineError>;

    /// Open a tab at `url`; answers its target id.
    fn open_tab(&mut self, url: &str) -> Result<String, EngineError>;

    /// Close a tab; its session's subscription ends.
    fn close_tab(&mut self, target_id: &str) -> Result<(), EngineError>;

    /// Bring a tab to the front.
    fn activate_tab(&mut self, target_id: &str) -> Result<(), EngineError>;

    /// Attach to a tab; answers the session id its commands and events use.
    fn attach(&mut self, target_id: &str) -> Result<String, EngineError>;

    /// Send a CDP command on a session and wait up to `timeout` for the answer.
    fn send(
        &self,
        session: &str,
        method: &str,
        params: Value,
        timeout: Duration,
    ) -> Result<Value, EngineError>;

    /// Send several commands at once and wait for all, in order. Engines that can pipeline do.
    fn send_many(
        &self,
        session: &str,
        calls: Vec<(String, Value)>,
        timeout: Duration,
    ) -> Vec<Result<Value, EngineError>> {
        calls
            .into_iter()
            .map(|(m, p)| self.send(session, &m, p, timeout))
            .collect()
    }

    /// [`Engine::send_many`], giving up on the answers still missing once `give_up` says so (it is asked every
    /// 10 ms or so; a missing answer is then [`EngineError::Interrupted`]): a page paused in a dialog does not answer
    /// the input that opened it. Engines that cannot give up wait as `send_many` does.
    fn send_many_unless(
        &self,
        session: &str,
        calls: Vec<(String, Value)>,
        timeout: Duration,
        give_up: &dyn Fn() -> bool,
    ) -> Vec<Result<Value, EngineError>> {
        let _ = give_up;
        self.send_many(session, calls, timeout)
    }

    /// The session's events. The channel disconnects when the tab closes or the browser goes away.
    fn subscribe(&self, session: &str) -> Result<mpsc::Receiver<CdpEvent>, EngineError>;

    /// The tab's frames, when the engine renders into memory the shell reads (the embedded engine): `record`'s
    /// source and the Web Browser window's.
    fn frames(&self, target_id: &str) -> Option<Arc<dyn FrameSource>> {
        let _ = target_id;
        None
    }

    /// The dialog or prompt the tab's page waits on, when the engine shows them itself (the embedded engine). `None`
    /// means none, or an engine that leaves dialogs to CDP (`Page.javascriptDialogOpening`).
    fn pending_dialog(&self, target_id: &str) -> Option<PendingDialog> {
        let _ = target_id;
        None
    }

    /// Answer [`Engine::pending_dialog`].
    fn answer_dialog(
        &self,
        target_id: &str,
        answer: &DialogAnswer,
    ) -> Result<PendingDialog, EngineError> {
        let _ = (target_id, answer);
        Err(EngineError::Launch(
            "this engine leaves dialogs to the Chrome DevTools Protocol".into(),
        ))
    }

    /// Open DevTools for the tab as a tab of the Web Browser window; true when it was not open yet.
    fn devtools(
        &mut self,
        target_id: &str,
        inspect: Option<(f64, f64)>,
    ) -> Result<bool, EngineError> {
        let _ = (target_id, inspect);
        Err(EngineError::Launch(
            "DevTools opens as a tab of the Web Browser window, which draws the embedded engine (the setting \
             browser.engine: embedded; tools/cef/fetch.sh fetches CEF); this browser is an external Chrome"
                .into(),
        ))
    }

    /// The tab's history and icon, when the engine tracks them (the embedded engine); `None`: ask the page.
    fn history(&self, target_id: &str) -> Option<TabHistory> {
        let _ = target_id;
        None
    }

    /// Encoded screenshot pixels (base64) for `Page.captureScreenshot`'s parameters.
    fn screenshot(
        &self,
        session: &str,
        params: Value,
        timeout: Duration,
    ) -> Result<String, EngineError> {
        let r = self.send(session, "Page.captureScreenshot", params, timeout)?;
        r["data"]
            .as_str()
            .map(str::to_owned)
            .ok_or_else(|| EngineError::Launch("Page.captureScreenshot answered no data".into()))
    }

    /// The browser's Chrome DevTools remote debugging endpoint, where a debugger attaches to its pages (brief 0038):
    /// the external Chrome's `--remote-debugging-port`, the embedded engine's (`engine/ready`). `None` while it does
    /// not run or has none. May wait briefly for an engine that just started to report it.
    fn debug_endpoint(&self) -> Option<DebugEndpoint> {
        None
    }

    /// The tab's target id as the debug endpoint lists it (`/json/list`'s `id`): the engine's own id unless the engine
    /// numbers its tabs itself (the embedded engine asks the page, `Target.getTargetInfo`). Brief 0038.
    fn cdp_target_id(&self, target_id: &str) -> Option<String> {
        Some(target_id.to_owned())
    }

    /// Close the browser (and its processes).
    fn shutdown(&mut self);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn devtools_endpoints() {
        assert_eq!(
            endpoint_of("ws://127.0.0.1:41235/devtools/browser/3c1b-77"),
            Some(DebugEndpoint {
                address: "127.0.0.1".into(),
                port: 41235
            })
        );
        assert_eq!(endpoint_of("stdio (pid 4)"), None);
        assert_eq!(endpoint_of("ws://127.0.0.1/devtools/browser/x"), None);
    }

    #[test]
    fn viewports() {
        assert_eq!(parse_viewport("1280x800"), Some((1280, 800)));
        assert_eq!(parse_viewport(" 390 X 844 "), Some((390, 844)));
        assert_eq!(parse_viewport("1280"), None);
        assert_eq!(parse_viewport("0x0"), None);
        assert_eq!(parse_viewport("wide"), None);
        assert_eq!(
            EngineConfig::profile_for(std::path::Path::new("/w")),
            std::path::Path::new("/w/.eludite/browser/profile")
        );
    }
}
