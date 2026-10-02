//! The engine trait: what the browser commands need from a browser, so the external Chrome of brief 0023, the
//! embedded CEF engine of brief B and a later Servo are implementations, not redesigns (ADR-0008).
//!
//! The commands speak the Chrome DevTools Protocol to a tab's session ([`Engine::send`]); an engine without CDP
//! would translate. Tabs are the engine's page targets; the command layer gives them short ids (`t1`) and keeps
//! their state ([`crate::tab`]).

use std::path::PathBuf;
use std::sync::mpsc;
use std::time::Duration;

use serde_json::Value;

use crate::connection::{CdpError, CdpEvent};

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
}

impl std::fmt::Display for EngineError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EngineError::NotRunning => write!(f, "the browser is not running"),
            EngineError::Launch(m) => write!(f, "{m}"),
            EngineError::Cdp(e) => write!(f, "{e}"),
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

    /// The session's events. The channel disconnects when the tab closes or the browser goes away.
    fn subscribe(&self, session: &str) -> Result<mpsc::Receiver<CdpEvent>, EngineError>;

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

    /// Close the browser (and its processes).
    fn shutdown(&mut self);
}

#[cfg(test)]
mod tests {
    use super::*;

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
