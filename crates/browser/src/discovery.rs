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
//!
//! Finding the embedded engine and its CEF (brief 0039), on first use only (the Web Browser window or a browser
//! command that needs the engine; never at startup), each search saying where it looked:
//!
//! - [`EngineSearch`], `eludite-chromium`: the setting `browser.enginePath`, `ELUDITE_CHROMIUM`, beside the running
//!   executable (a packaged Eludite: `tools/package/linux.sh` puts the engine beside `eludite`), then cargo's build
//!   layout in a development build (the test binary's `target/<profile>/`, `CARGO_TARGET_DIR`, the repository's
//!   `target/`).
//! - [`CefSearch`], CEF's folder: beside the engine (`libcef.so` in the engine's folder, as cargo copies it, or in
//!   `cef/` beside it, as the package lays it out), `ELUDITE_CEF` and `CEF_PATH`, then `tools/cef/fetch.sh`'s cache
//!   for the version of `tools/cef/PIN`.
//!
//! The first found wins; [`EngineFound`] and [`CefFound`] say which (`eludite.browser.tabs`' `engine.found_by`).

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

// ---- the embedded engine and CEF (brief 0039) ----

/// How [`EngineSearch`] found the engine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EngineFound {
    /// `browser.enginePath`.
    Setting,
    /// `ELUDITE_CHROMIUM`.
    Variable,
    /// Beside the running executable.
    Beside,
    /// Cargo's build layout (a development build).
    Dev,
}

impl EngineFound {
    /// `browser-tabs.output.json`'s `engine.found_by.engine`.
    pub fn as_str(self) -> &'static str {
        match self {
            EngineFound::Setting => "setting",
            EngineFound::Variable => "variable",
            EngineFound::Beside => "beside",
            EngineFound::Dev => "dev",
        }
    }
}

/// How [`CefSearch`] found CEF.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CefFound {
    /// Beside the engine (its folder, or `cef/` beside it).
    Beside,
    /// `ELUDITE_CEF` or `CEF_PATH`.
    Variable,
    /// `tools/cef/fetch.sh`'s cache.
    Cache,
}

impl CefFound {
    /// `browser-tabs.output.json`'s `engine.found_by.cef`.
    pub fn as_str(self) -> &'static str {
        match self {
            CefFound::Beside => "beside",
            CefFound::Variable => "variable",
            CefFound::Cache => "cache",
        }
    }
}

/// Where to look for `eludite-chromium`, in order.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EngineSearch {
    /// `browser.enginePath`.
    pub setting: Option<PathBuf>,
    /// `ELUDITE_CHROMIUM`.
    pub variable: Option<PathBuf>,
    /// The running executable's folder.
    pub beside: Option<PathBuf>,
    /// Cargo's build folders, searched in a development build only.
    pub dev: Vec<PathBuf>,
}

/// Where to look for CEF, after the folder beside the engine.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CefSearch {
    /// `ELUDITE_CEF`, then `CEF_PATH`.
    pub variables: Vec<PathBuf>,
    /// `tools/cef/fetch.sh`'s cache: `~/.cache/eludite/cef/<version>` (`CEF_CACHE` replaces `~/.cache/eludite/cef`).
    pub cache: Option<PathBuf>,
}

/// The engine's executable name on this platform.
pub fn engine_file_name() -> String {
    format!(
        "{}{}",
        crate::embedded::ENGINE_NAME,
        std::env::consts::EXE_SUFFIX
    )
}

/// The file whose presence makes a folder CEF's.
pub fn cef_library() -> &'static str {
    if cfg!(windows) {
        "libcef.dll"
    } else if cfg!(target_os = "macos") {
        "Chromium Embedded Framework.framework"
    } else {
        "libcef.so"
    }
}

fn env_path(name: &str) -> Option<PathBuf> {
    std::env::var_os(name)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
}

impl EngineSearch {
    /// The setting's value (`None` when empty), `ELUDITE_CHROMIUM`, the running executable's folder and, in a
    /// development build, cargo's build folders.
    pub fn from_env(setting: Option<PathBuf>) -> Self {
        let exe_dir = std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(Path::to_path_buf));
        let mut dev = Vec::new();
        if cfg!(debug_assertions) {
            // A test binary sits in target/<profile>/deps; the engine in target/<profile>.
            if let Some(dir) = &exe_dir
                && dir.file_name().is_some_and(|n| n == "deps")
                && let Some(up) = dir.parent()
            {
                dev.push(up.to_path_buf());
            }
            if let Some(t) = env_path("CARGO_TARGET_DIR") {
                dev.push(t.join("debug"));
            }
            // The repository this crate was built from (crates/browser/../../target/debug).
            dev.push(
                Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("..")
                    .join("..")
                    .join("target")
                    .join("debug"),
            );
        }
        Self {
            setting: setting.filter(|p| !p.as_os_str().is_empty()),
            variable: env_path(crate::embedded::ENGINE_ENV),
            beside: exe_dir,
            dev,
        }
    }

    /// The engine, how it was found, or a message naming every place looked, the fetch script and the package.
    pub fn find(&self) -> Result<(PathBuf, EngineFound), String> {
        let name = engine_file_name();
        let named = [
            (&self.setting, EngineFound::Setting, "browser.enginePath"),
            (
                &self.variable,
                EngineFound::Variable,
                crate::embedded::ENGINE_ENV,
            ),
        ];
        // An engine named explicitly must be there: never a silent fallback to another one.
        for (p, how, what) in named {
            if let Some(p) = p {
                return if p.is_file() {
                    Ok((p.clone(), how))
                } else {
                    Err(format!(
                        "{what} names {}, which does not exist",
                        p.display()
                    ))
                };
            }
        }
        let mut looked = vec![
            "browser.enginePath: empty".to_owned(),
            format!("{}: unset", crate::embedded::ENGINE_ENV),
        ];
        if let Some(dir) = &self.beside {
            let p = dir.join(&name);
            if p.is_file() {
                return Ok((p, EngineFound::Beside));
            }
            looked.push(format!("beside eludite: {}", p.display()));
        }
        for dir in &self.dev {
            let p = dir.join(&name);
            if p.is_file() {
                return Ok((p, EngineFound::Dev));
            }
            looked.push(format!("cargo's build folder: {}", p.display()));
        }
        Err(format!(
            "{} was not found (looked at {}). A packaged Eludite (tools/package/linux.sh) has it beside eludite; in a \
             checkout, fetch CEF with tools/cef/fetch.sh and build it with `CEF_PATH=\"$(tools/cef/fetch.sh)\" cargo \
             build -p eludite-chromium --features eludite-chromium/cef`",
            crate::embedded::ENGINE_NAME,
            looked.join("; ")
        ))
    }
}

impl CefSearch {
    /// `ELUDITE_CEF`, `CEF_PATH` and the fetch script's cache for the pinned version.
    pub fn from_env() -> Self {
        let cache_root = env_path("CEF_CACHE").or_else(|| {
            std::env::var_os("HOME")
                .filter(|h| !h.is_empty())
                .or_else(|| std::env::var_os("USERPROFILE").filter(|h| !h.is_empty()))
                .map(|h| PathBuf::from(h).join(".cache").join("eludite").join("cef"))
        });
        Self {
            variables: [crate::embedded::CEF_ENV, "CEF_PATH"]
                .iter()
                .filter_map(|v| env_path(v))
                .collect(),
            cache: cache_root.map(|r| r.join(crate::embedded::CEF_VERSION)),
        }
    }

    /// CEF's folder for the engine at `engine`, how it was found, or a message naming every place looked.
    pub fn find(&self, engine: &Path) -> Result<(PathBuf, CefFound), String> {
        let lib = cef_library();
        let mut looked = Vec::new();
        let engine_dir = engine.parent().map(Path::to_path_buf).unwrap_or_default();
        for dir in [engine_dir.clone(), engine_dir.join("cef")] {
            if dir.join(lib).exists() {
                return Ok((dir, CefFound::Beside));
            }
            looked.push(format!("beside the engine: {}", dir.join(lib).display()));
        }
        for dir in &self.variables {
            if dir.join(lib).exists() {
                return Ok((dir.clone(), CefFound::Variable));
            }
            looked.push(format!(
                "{} or CEF_PATH: {}",
                crate::embedded::CEF_ENV,
                dir.join(lib).display()
            ));
        }
        if self.variables.is_empty() {
            looked.push(format!("{} and CEF_PATH: unset", crate::embedded::CEF_ENV));
        }
        if let Some(dir) = &self.cache {
            if dir.join(lib).exists() {
                return Ok((dir.clone(), CefFound::Cache));
            }
            looked.push(format!("tools/cef/fetch.sh's cache: {}", dir.display()));
        }
        Err(format!(
            "CEF {} was not found for {} (looked at {}); run tools/cef/fetch.sh, or use the package layout \
             (tools/package/linux.sh), which ships CEF in cef/ beside the engine",
            crate::embedded::CEF_VERSION,
            engine.display(),
            looked.join("; ")
        ))
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

    /// Brief 0039: the engine's order (setting, variable, beside the executable, cargo's layout) and CEF's (beside the
    /// engine or in `cef/` beside it, the variables, the cache), with temporary folders; the messages when nothing is
    /// found name every place looked, the fetch script and the package.
    #[test]
    fn the_engine_and_cef_are_searched_in_order() {
        let t = tempfile::tempdir().unwrap();
        let root = t.path();
        let dir = |n: &str| {
            let d = root.join(n);
            std::fs::create_dir_all(&d).unwrap();
            d
        };
        let (setting, variable, beside, dev) =
            (dir("setting"), dir("variable"), dir("beside"), dir("dev"));
        let name = engine_file_name();
        let mut s = EngineSearch {
            setting: None,
            variable: None,
            beside: Some(beside.clone()),
            dev: vec![dev.clone()],
        };
        let e = s.find().unwrap_err();
        assert!(
            e.contains("tools/cef/fetch.sh") && e.contains("tools/package/linux.sh"),
            "{e}"
        );
        assert!(e.contains(&beside.join(&name).display().to_string()), "{e}");
        assert!(e.contains(&dev.join(&name).display().to_string()), "{e}");
        assert!(
            e.contains("browser.enginePath") && e.contains("ELUDITE_CHROMIUM"),
            "{e}"
        );
        std::fs::write(dev.join(&name), b"").unwrap();
        assert_eq!(s.find().unwrap(), (dev.join(&name), EngineFound::Dev));
        std::fs::write(beside.join(&name), b"").unwrap();
        assert_eq!(s.find().unwrap(), (beside.join(&name), EngineFound::Beside));
        std::fs::write(variable.join(&name), b"").unwrap();
        s.variable = Some(variable.join(&name));
        assert_eq!(
            s.find().unwrap(),
            (variable.join(&name), EngineFound::Variable)
        );
        std::fs::write(setting.join(&name), b"").unwrap();
        s.setting = Some(setting.join(&name));
        assert_eq!(
            s.find().unwrap(),
            (setting.join(&name), EngineFound::Setting)
        );
        // Named but missing: an error, not the next place.
        s.setting = Some(root.join("missing"));
        assert!(s.find().unwrap_err().contains("browser.enginePath names"));
        s.setting = None;
        s.variable = Some(root.join("missing"));
        assert!(s.find().unwrap_err().contains("ELUDITE_CHROMIUM names"));

        let lib = cef_library();
        let (cef_var, cache) = (dir("cef-var"), dir("cache"));
        let engine = beside.join(&name);
        let c = CefSearch {
            variables: vec![cef_var.clone()],
            cache: Some(cache.clone()),
        };
        let e = c.find(&engine).unwrap_err();
        assert!(
            e.contains("tools/cef/fetch.sh") && e.contains("tools/package/linux.sh"),
            "{e}"
        );
        assert!(e.contains(crate::embedded::CEF_VERSION), "{e}");
        assert!(
            e.contains(&beside.join("cef").join(lib).display().to_string()),
            "{e}"
        );
        assert!(e.contains(&cache.display().to_string()), "{e}");
        std::fs::write(cache.join(lib), b"").unwrap();
        assert_eq!(c.find(&engine).unwrap(), (cache.clone(), CefFound::Cache));
        std::fs::write(cef_var.join(lib), b"").unwrap();
        assert_eq!(
            c.find(&engine).unwrap(),
            (cef_var.clone(), CefFound::Variable)
        );
        std::fs::create_dir_all(beside.join("cef")).unwrap();
        std::fs::write(beside.join("cef").join(lib), b"").unwrap();
        assert_eq!(
            c.find(&engine).unwrap(),
            (beside.join("cef"), CefFound::Beside),
            "the package layout: cef/ beside the engine"
        );
        std::fs::write(beside.join(lib), b"").unwrap();
        assert_eq!(
            c.find(&engine).unwrap(),
            (beside.clone(), CefFound::Beside),
            "cargo's layout: libcef.so beside the engine"
        );
    }

    /// The budget: discovery beside the executable takes well under 1 ms (printed; asserted under 50 ms so a loaded
    /// machine does not fail it).
    #[test]
    fn discovery_beside_the_executable_is_quick() {
        let t = tempfile::tempdir().unwrap();
        let name = engine_file_name();
        std::fs::write(t.path().join(&name), b"").unwrap();
        std::fs::create_dir_all(t.path().join("cef")).unwrap();
        std::fs::write(t.path().join("cef").join(cef_library()), b"").unwrap();
        let s = EngineSearch {
            setting: None,
            variable: None,
            beside: Some(t.path().to_path_buf()),
            dev: Vec::new(),
        };
        let c = CefSearch::default();
        let mut times = Vec::new();
        for _ in 0..200 {
            let at = std::time::Instant::now();
            let (engine, _) = s.find().unwrap();
            let (cef, how) = c.find(&engine).unwrap();
            times.push(at.elapsed());
            assert_eq!((cef, how), (t.path().join("cef"), CefFound::Beside));
        }
        times.sort();
        let p50 = times[times.len() / 2];
        let max = times[times.len() - 1];
        eprintln!("discovery beside the executable: p50 {p50:?}, max {max:?} over 200");
        crate::assert_budget("discovery p50", p50, std::time::Duration::from_millis(50));
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
