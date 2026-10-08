//! Eludite's browser automation (briefs 0023 and 0024, proposal 0002, ADR-0008): the `eludite.browser.*` commands against a
//! browser engine, over the Chrome DevTools Protocol.
//!
//! Public API:
//!
//! - [`Browser`]: runs a parsed [`eludite_commands::browser::BrowserRequest`] and answers its output. It owns the
//!   engine and the tabs; one thread owns it (the shell's `browser` worker) and every call may wait on the engine.
//! - [`Engine`]: the engine trait (open, close, select and attach tabs, send a CDP command on a tab's session,
//!   subscribe to its events, screenshot pixels, shutdown), with [`EngineConfig`] for the next launch. The embedded
//!   CEF engine and a later Servo implement it; tests implement fakes.
//! - [`ExternalChrome`]: the implementation of this brief, a Chrome or Chromium process launched with the
//!   workspace's own profile ([`chrome`]), found by [`ChromeSearch`] ([`discovery`]).
//! - [`EmbeddedChromium`]: the second engine (brief 0031), `eludite-chromium` (CEF) in its own process over JSON-RPC
//!   on stdio, its tabs rendered into shared memory ([`FrameSource`], [`TabFrames`]), found by [`ChromiumSearch`]
//!   ([`embedded`]): [`EngineSearch`] and [`CefSearch`] ([`discovery`], brief 0039: the package layout beside the
//!   executable first, on first use only). Nothing of CEF loads in this process.
//! - [`connection`]: the websocket CDP client (`tungstenite` over `std::net::TcpStream`, one reader thread per
//!   connection, requests correlated by id with timeouts, events fanned out per session through channels).
//! - [`ring`], [`tab`] and [`page`]: the console and network rings, a tab's state (page generations and refs), and
//!   reading pages from CDP answers.
//! - Brief 0032, the Web Browser window: [`EngineEvent`]s and [`TabControl`] for the window that draws the embedded
//!   engine's tabs, [`select_engine`] for the setting `browser.engine`, the engine's dialogs ([`PendingDialog`],
//!   [`DialogAnswer`]), `record` (`browser::record`), `devtools`, `dialog`, and the person's [`Interrupt`].
//! - [`keys`]: the key table of `eludite.browser.input` (brief 0024), and the Web Browser window's
//!   (`keys::SHELL_KEYS`, brief 0032). Acting on the page (`input`, `form_input`,
//!   `upload`), `storage`, `network_body` and `open_external` (whose opener [`browser::OPENER_ENV`] replaces in
//!   tests) are [`Browser`] commands like the rest.
//!
//! No GPUI and no async runtime: everything here is threads and channels, and `Send`. The CDP domain types are
//! generated (`eludite-protocol`'s `cdp` module, from `protocol/cdp/`); the message envelope is typed in
//! [`connection`].

pub mod browser;
pub mod chrome;
pub mod connection;
pub mod discovery;
pub mod embedded;
pub mod engine;
pub mod keys;
pub mod page;
mod process;
pub mod ring;
pub mod tab;

/// `measured` under `limit`, asserted on a developer machine only: under CI (`CI` set) the hosted runners are shared
/// VMs, not a reference machine, so the number is printed instead.
#[cfg(test)]
pub(crate) fn assert_budget(what: &str, measured: std::time::Duration, limit: std::time::Duration) {
    if std::env::var_os("CI").is_some() {
        eprintln!(
            "timing: {what} {:.2} ms not asserted against {:.0} ms: a CI run, not a reference machine",
            measured.as_secs_f64() * 1e3,
            limit.as_secs_f64() * 1e3
        );
    } else {
        assert!(
            measured < limit,
            "{what}: {measured:?} is not under {limit:?}"
        );
    }
}

pub use browser::{Browser, DebugTarget};
pub use browser::{DebuggerPauses, Interrupt, Marks};
pub use chrome::ExternalChrome;
pub use connection::{CdpError, CdpEvent, Connection};
pub use discovery::{CefFound, CefSearch, ChromeSearch, EngineFound, EngineSearch};
pub use embedded::{
    ChromiumSearch, Discovered, EmbeddedChromium, EmbeddedStats, EngineChoice, EngineEvent,
    EngineObserver, FrameSource, TabControl, TabFrames, TabInfo, select_engine,
};
pub use engine::{
    DebugEndpoint, DialogAnswer, Engine, EngineConfig, EngineError, LaunchInfo, PendingDialog,
    TabHistory, TargetInfo,
};

/// Where the browser's lifecycle lines go (the shell's Output window, Browser source). Called from any thread;
/// it must not block.
pub type LogSink = std::sync::Arc<dyn Fn(&str) + Send + Sync>;
