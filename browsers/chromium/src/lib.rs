//! `eludite-chromium` (brief 0031, proposal 0002 section 3, ADR-0008): CEF's browser process for Eludite's embedded
//! browser. The shell starts it the first time a browser tab needs it; it renders each tab off screen (CEF's
//! windowless mode, `OnPaint` with BGRA pixels) into a shared-memory ring the shell maps, and takes its orders as
//! JSON-RPC on stdin (`protocol/schemas/browser-rpc/`). CEF's renderer, GPU and utility subprocesses are this same
//! executable started with `--type=`.
//!
//! Public API (for the binary and the engine's tests; the shell talks to the process, never links this crate):
//!
//! - [`shm`]: the frame ring's layout, the engine's writer ([`shm::Region`]), a reader ([`shm::Reader`]) and the
//!   descriptor passing over the fd-3 socket.
//! - [`rpc`]: the control channel's messages and framing.
//! - [`privacy`]: the switches, feature flags and profile preferences that keep Chrome's background services from
//!   making requests nobody asked for (brief 0032).
//! - [`sandbox`]: whether the engine may start sandboxed, refuses, or runs with `--no-sandbox` because
//!   `ELUDITE_CHROME_NO_SANDBOX=1` asked for it.
//! - `engine` (feature `cef`): CEF itself: the app, the tabs, the render handler, the DevTools pass-through.
//!
//! The process's stdout carries the control protocol only: the engine duplicates it for the protocol and points
//! descriptor 1 at stderr before CEF starts, so nothing else can write there.

pub mod privacy;
pub mod rpc;
pub mod sandbox;
#[cfg(unix)]
pub mod shm;

#[cfg(all(feature = "cef", target_os = "linux"))]
pub mod engine;

/// The executable's name.
pub const NAME: &str = "eludite-chromium";

/// The engine's options (its command line, beside CEF's own switches).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Options {
    /// `--profile DIR`: the root cache path (the workspace's `.eludite/browser/profile`).
    pub profile: Option<std::path::PathBuf>,
    /// `--cef-dir DIR`: CEF's resources and locales; the executable's folder when absent.
    pub cef_dir: Option<std::path::PathBuf>,
    /// `--frame-socket FD`: the Unix socket the region descriptors go over (3 by default).
    pub frame_socket: Option<i32>,
}

impl Options {
    /// Our switches, wherever they are among CEF's (`--name value` or `--name=value`).
    pub fn parse(args: impl IntoIterator<Item = String>) -> Options {
        let mut o = Options::default();
        let mut it = args.into_iter().peekable();
        while let Some(a) = it.next() {
            let (name, inline) = match a.split_once('=') {
                Some((n, v)) => (n.to_owned(), Some(v.to_owned())),
                None => (a.clone(), None),
            };
            let mut value = || inline.clone().or_else(|| it.next());
            match name.as_str() {
                "--profile" => o.profile = value().map(Into::into),
                "--cef-dir" => o.cef_dir = value().map(Into::into),
                "--frame-socket" => o.frame_socket = value().and_then(|v| v.parse().ok()),
                _ => {}
            }
        }
        o
    }

    /// Whether this is one of CEF's subprocesses (`--type=renderer`, `gpu-process`, `utility`, `zygote`, ...).
    pub fn is_subprocess(args: &[String]) -> bool {
        args.iter().any(|a| a.starts_with("--type="))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn options() {
        let o = Options::parse(
            [
                "eludite-chromium",
                "--profile",
                "/w/.eludite/browser/profile",
                "--cef-dir=/c",
                "--frame-socket",
                "3",
                "--enable-logging=stderr",
            ]
            .map(String::from),
        );
        assert_eq!(
            o.profile.as_deref(),
            Some(std::path::Path::new("/w/.eludite/browser/profile"))
        );
        assert_eq!(o.cef_dir.as_deref(), Some(std::path::Path::new("/c")));
        assert_eq!(o.frame_socket, Some(3));
        assert!(Options::is_subprocess(&[
            "x".into(),
            "--type=renderer".into()
        ]));
        assert!(!Options::is_subprocess(&["x".into()]));
    }
}
