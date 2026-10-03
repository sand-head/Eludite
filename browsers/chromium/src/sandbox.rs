//! The sandbox rule of brief 0031 (and brief 0023's variable): on Linux the engine runs with Chromium's sandbox when
//! CEF's `chrome-sandbox` helper is owned by root with the setuid bit, and otherwise refuses to start, naming the
//! helper. `ELUDITE_CHROME_NO_SANDBOX=1` is the only way to run with `--no-sandbox`; nothing adds it silently.

use std::path::{Path, PathBuf};

/// brief 0023's opt-in.
pub const NO_SANDBOX_ENV: &str = "ELUDITE_CHROME_NO_SANDBOX";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    /// Run sandboxed; on Linux through this setuid helper (`CHROME_DEVEL_SANDBOX`).
    Sandboxed { helper: Option<PathBuf> },
    /// `--no-sandbox`, because the variable asked for it.
    NoSandbox,
}

/// Decide from the variable's value and CEF's folder.
pub fn decide(no_sandbox_env: Option<&str>, cef_dir: &Path) -> Result<Decision, String> {
    if no_sandbox_env == Some("1") {
        return Ok(Decision::NoSandbox);
    }
    if !cfg!(target_os = "linux") {
        // Windows and macOS sandbox setup is packaging (proposal 0002, brief E); the spike runs Linux only.
        return Ok(Decision::Sandboxed { helper: None });
    }
    let helper = cef_dir.join("chrome-sandbox");
    match setuid_root(&helper) {
        Some(true) => Ok(Decision::Sandboxed {
            helper: Some(helper),
        }),
        found => Err(format!(
            "refusing to start without Chromium's sandbox: the setuid helper {} {}. Install it with \
             `sudo chown root:root {p} && sudo chmod 4755 {p}`, or set {NO_SANDBOX_ENV}=1 to run without the \
             sandbox (needed when running as root).",
            helper.display(),
            if found.is_none() {
                "is missing"
            } else {
                "is not owned by root with the setuid bit (mode 4755)"
            },
            p = helper.display(),
        )),
    }
}

#[cfg(unix)]
fn setuid_root(p: &Path) -> Option<bool> {
    use std::os::unix::fs::MetadataExt;
    let m = std::fs::metadata(p).ok()?;
    Some(m.uid() == 0 && m.mode() & 0o4000 != 0)
}

#[cfg(not(unix))]
fn setuid_root(_: &Path) -> Option<bool> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_variable_is_the_only_way_to_no_sandbox() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(decide(Some("1"), dir.path()), Ok(Decision::NoSandbox));
        if cfg!(target_os = "linux") {
            let e = decide(None, dir.path()).unwrap_err();
            assert!(
                e.contains("chrome-sandbox") && e.contains("is missing"),
                "{e}"
            );
            assert!(e.contains(NO_SANDBOX_ENV));
            assert!(decide(Some("0"), dir.path()).is_err());
            // A helper that is not setuid root (a copy, as cargo's build leaves it) is refused too.
            std::fs::write(dir.path().join("chrome-sandbox"), b"").unwrap();
            let e = decide(None, dir.path()).unwrap_err();
            assert!(e.contains("4755"), "{e}");
        }
    }
}
