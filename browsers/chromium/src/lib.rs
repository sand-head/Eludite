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
//! - [`window`]: the Web Browser window's helpers that need no CEF (popup compositing, cursor and permission names,
//!   download paths).
//! - [`sandbox`]: the sandbox rule (brief 0039): user namespaces, else the setuid helper, else a refusal naming both
//!   remedies; `--no-sandbox` only with `--allow-no-sandbox` on the command line.
//! - [`cef_dir_beside`]: where the engine finds CEF when the shell names no `--cef-dir`: its own folder (cargo's
//!   build layout), or `cef/` beside it (the package layout of `tools/package/linux.sh`).
//! - `engine` (feature `cef`): CEF itself: the app, the tabs, the render handler, the DevTools pass-through.
//!
//! The process's stdout carries the control protocol only: the engine duplicates it for the protocol and points
//! descriptor 1 at stderr before CEF starts, so nothing else can write there.

pub mod privacy;
pub mod rpc;
pub mod sandbox;
#[cfg(unix)]
pub mod shm;
pub mod window;

#[cfg(all(feature = "cef", target_os = "linux"))]
pub mod engine;

/// The executable's name.
pub const NAME: &str = "eludite-chromium";

/// The file whose presence makes a folder CEF's.
pub const CEF_LIBRARY: &str = "libcef.so";

/// CEF's folder for the engine at `exe` when no `--cef-dir` names it: the engine's own folder when `libcef.so` is
/// there (cargo copies CEF's runtime files beside the executable), else `cef/` beside the engine when it holds
/// `libcef.so` (the package layout), else the engine's folder.
pub fn cef_dir_beside(exe: &std::path::Path) -> std::path::PathBuf {
    let dir = exe
        .parent()
        .map(std::path::Path::to_path_buf)
        .unwrap_or_default();
    let packaged = dir.join("cef");
    if !dir.join(CEF_LIBRARY).exists() && packaged.join(CEF_LIBRARY).exists() {
        packaged
    } else {
        dir
    }
}

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
    fn cef_beside_the_engine_or_in_cef() {
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join(NAME);
        assert_eq!(
            cef_dir_beside(&exe),
            dir.path(),
            "nothing: the engine's folder"
        );
        std::fs::create_dir_all(dir.path().join("cef")).unwrap();
        std::fs::write(dir.path().join("cef").join(CEF_LIBRARY), b"").unwrap();
        assert_eq!(
            cef_dir_beside(&exe),
            dir.path().join("cef"),
            "the package layout"
        );
        std::fs::write(dir.path().join(CEF_LIBRARY), b"").unwrap();
        assert_eq!(cef_dir_beside(&exe), dir.path(), "cargo's layout first");
    }

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
                "--allow-no-sandbox",
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
