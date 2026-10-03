//! Finding Chrome or Chromium (brief 0023), in order:
//!
//! 1. the setting `browser.chromePath` (the environment variable `ELUDITE_CHROME` overrides it);
//! 2. Chrome for Testing from `tools/chrome/fetch.sh`: `~/.cache/eludite/chrome/<version>/chrome-<platform>/...`;
//! 3. on `PATH`: `google-chrome`, `google-chrome-stable`, `chromium`, `chromium-browser`, `chrome`;
//! 4. the platform's usual install locations: `/Applications/Google Chrome.app/...` on macOS,
//!    `%ProgramFiles%`, `%ProgramFiles(x86)%` and `%LocalAppData%` `\Google\Chrome\Application\chrome.exe` on
//!    Windows, `/opt/google/chrome/chrome` on Linux.
//!
//! When none is found the error names every place searched and `tools/chrome/fetch.sh`.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// The Chrome for Testing version `tools/chrome/PIN` pins.
pub const PINNED_VERSION: &str = "154.0.8037.92";

/// Executable names looked up on `PATH`, in order.
pub const PATH_NAMES: [&str; 5] = [
    "google-chrome",
    "google-chrome-stable",
    "chromium",
    "chromium-browser",
    "chrome",
];

/// Where to look.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ChromeSearch {
    /// `browser.chromePath` (or `ELUDITE_CHROME`).
    pub configured: Option<PathBuf>,
    /// The fetch script's cache root (`~/.cache/eludite/chrome`).
    pub cache_root: Option<PathBuf>,
    /// The `PATH` value.
    pub path: Option<OsString>,
    /// The platform's install locations, full paths to the executable.
    pub installs: Vec<PathBuf>,
}

/// The executable inside the fetch script's cache, relative to `<cache root>/<version>/`.
pub fn cache_relative() -> &'static str {
    if cfg!(windows) {
        "chrome-win64/chrome.exe"
    } else if cfg!(target_os = "macos") {
        if cfg!(target_arch = "aarch64") {
            "chrome-mac-arm64/Google Chrome for Testing.app/Contents/MacOS/Google Chrome for Testing"
        } else {
            "chrome-mac-x64/Google Chrome for Testing.app/Contents/MacOS/Google Chrome for Testing"
        }
    } else {
        "chrome-linux64/chrome"
    }
}

/// `~/.cache/eludite/chrome` (`%USERPROFILE%\.cache\eludite\chrome` on Windows), as the fetch scripts use.
pub fn default_cache_root() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")
        .filter(|h| !h.is_empty())
        .or_else(|| std::env::var_os("USERPROFILE").filter(|h| !h.is_empty()))?;
    Some(
        PathBuf::from(home)
            .join(".cache")
            .join("eludite")
            .join("chrome"),
    )
}

/// The platform's usual install locations.
pub fn default_installs() -> Vec<PathBuf> {
    if cfg!(windows) {
        ["ProgramFiles", "ProgramFiles(x86)", "LocalAppData"]
            .iter()
            .filter_map(std::env::var_os)
            .map(|base| PathBuf::from(base).join(r"Google\Chrome\Application\chrome.exe"))
            .collect()
    } else if cfg!(target_os = "macos") {
        vec![PathBuf::from(
            "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
        )]
    } else {
        vec![PathBuf::from("/opt/google/chrome/chrome")]
    }
}

impl ChromeSearch {
    /// `ELUDITE_CHROME`, the default cache, `PATH` and the install locations of this machine.
    pub fn from_env() -> Self {
        Self {
            configured: std::env::var_os("ELUDITE_CHROME")
                .filter(|v| !v.is_empty())
                .map(PathBuf::from),
            ..Self::defaults()
        }
    }

    /// As [`ChromeSearch::from_env`] without `ELUDITE_CHROME` (the shell passes the setting, which already
    /// resolved it).
    pub fn defaults() -> Self {
        Self {
            configured: None,
            cache_root: default_cache_root(),
            path: std::env::var_os("PATH"),
            installs: default_installs(),
        }
    }

    /// Every place searched, in order, with where it came from.
    pub fn candidates(&self) -> Vec<(String, PathBuf)> {
        let mut out = Vec::new();
        if let Some(p) = &self.configured {
            out.push(("browser.chromePath (ELUDITE_CHROME)".to_owned(), p.clone()));
        }
        if let Some(root) = &self.cache_root {
            out.push((
                "Chrome for Testing from tools/chrome/fetch.sh".to_owned(),
                root.join(PINNED_VERSION).join(cache_relative()),
            ));
        }
        if let Some(path) = &self.path {
            // Each name through the whole PATH before the next name, as `which` would find it.
            for name in PATH_NAMES {
                let file = if cfg!(windows) {
                    format!("{name}.exe")
                } else {
                    name.to_owned()
                };
                for dir in std::env::split_paths(path) {
                    out.push((format!("PATH ({name})"), dir.join(&file)));
                }
            }
        }
        for p in &self.installs {
            out.push(("install location".to_owned(), p.clone()));
        }
        out
    }

    /// The first executable found, or a message naming every place searched.
    pub fn find(&self) -> Result<PathBuf, String> {
        if let Some(p) = &self.configured
            && !is_executable(p)
        {
            return Err(format!(
                "browser.chromePath (or ELUDITE_CHROME) is {}, which is not an executable file",
                p.display()
            ));
        }
        let candidates = self.candidates();
        if let Some((_, p)) = candidates.iter().find(|(_, p)| is_executable(p)) {
            return Ok(p.clone());
        }
        let mut msg = String::from(
            "no Chrome or Chromium found. Run tools/chrome/fetch.sh (tools\\chrome\\fetch.ps1 on Windows) to install Chrome for Testing, or set browser.chromePath (ELUDITE_CHROME). Searched:",
        );
        if self.configured.is_none() {
            msg.push_str("\n  browser.chromePath (ELUDITE_CHROME): not set");
        }
        if let Some(root) = &self.cache_root {
            msg.push_str(&format!(
                "\n  {}",
                root.join(PINNED_VERSION).join(cache_relative()).display()
            ));
        }
        msg.push_str(&format!("\n  PATH: {}", PATH_NAMES.join(", ")));
        for p in &self.installs {
            msg.push_str(&format!("\n  {}", p.display()));
        }
        Err(msg)
    }
}

/// A regular file that is executable (on Unix, any execute bit).
pub fn is_executable(p: &Path) -> bool {
    let Ok(meta) = std::fs::metadata(p) else {
        return false;
    };
    if !meta.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        meta.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn exe(dir: &Path, rel: &str) -> PathBuf {
        let p = dir.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, "#!/bin/sh\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        p
    }

    #[test]
    fn searches_in_order() {
        let t = tempfile::tempdir().unwrap();
        let root = t.path();
        let bin1 = root.join("bin1");
        let bin2 = root.join("bin2");
        std::fs::create_dir_all(&bin1).unwrap();
        std::fs::create_dir_all(&bin2).unwrap();
        let path = std::env::join_paths([&bin1, &bin2]).unwrap();
        let ext = if cfg!(windows) { ".exe" } else { "" };
        let mut search = ChromeSearch {
            configured: None,
            cache_root: Some(root.join("cache")),
            path: Some(path),
            installs: vec![root.join("opt/google/chrome/chrome")],
        };
        // Nothing yet: the message names every place and the fetch script.
        let err = search.find().unwrap_err();
        assert!(err.contains("tools/chrome/fetch.sh"), "{err}");
        assert!(err.contains("google-chrome-stable"), "{err}");
        assert!(
            err.contains(&root.join("opt/google/chrome/chrome").display().to_string()),
            "{err}"
        );
        assert!(
            err.contains(
                &root
                    .join("cache")
                    .join(PINNED_VERSION)
                    .display()
                    .to_string()
            ),
            "{err}"
        );
        // Each layer wins over the ones after it.
        let install = exe(root, "opt/google/chrome/chrome");
        assert_eq!(search.find().unwrap(), install);
        let chrome_in_bin2 = exe(&bin2, &format!("chrome{ext}"));
        assert_eq!(search.find().unwrap(), chrome_in_bin2);
        // Names in order, each through the whole PATH: google-chrome in the second directory beats chromium in the
        // first.
        exe(&bin1, &format!("chromium{ext}"));
        let google = exe(&bin2, &format!("google-chrome{ext}"));
        assert_eq!(search.find().unwrap(), google);
        let cached = exe(&root.join("cache").join(PINNED_VERSION), cache_relative());
        assert_eq!(search.find().unwrap(), cached);
        let configured = exe(root, "custom/chrome");
        search.configured = Some(configured.clone());
        assert_eq!(search.find().unwrap(), configured);
        // A configured path that is not there is an error, not a silent fallback.
        search.configured = Some(root.join("missing"));
        assert!(search.find().unwrap_err().contains("not an executable"));
    }

    #[cfg(unix)]
    #[test]
    fn a_file_without_execute_bits_is_skipped() {
        let t = tempfile::tempdir().unwrap();
        let p = t.path().join("chrome");
        std::fs::write(&p, "").unwrap();
        assert!(!is_executable(&p));
        assert!(!is_executable(t.path()));
    }
}
