//! Locating the debug adapters and the runtimes they need; each is located on the machine, never vendored into git.
//!
//! netcoredbg (MIT, https://github.com/Samsung/netcoredbg; `tools/netcoredbg/fetch.sh` downloads a pinned release),
//! [`AdapterSearch`]:
//!
//! 1. bundled beside the Eludite executable: `<exe dir>/netcoredbg/netcoredbg` (the release archive's folder), then
//!    `<exe dir>/netcoredbg`;
//! 2. `ELUDITE_NETCOREDBG`: the executable, or the folder holding it;
//! 3. `PATH`.
//!
//! The executable is `netcoredbg.exe` on Windows.
//!
//! Mono (brief 0022; the order brief 0003 fixed in `dotnet/src/Eludite.Host/Legacy/MonoInstallation.cs`),
//! [`MonoSearch`]: the setting `debugger.monoPrefix` (`ELUDITE_MONO_PREFIX`), `mono` on `PATH` (its prefix is two
//! folders up from the resolved executable), `~/.local/opt/mono-root/usr`, `/usr`, `/usr/local`,
//! `/Library/Frameworks/Mono.framework/Versions/Current`; a prefix counts when `<prefix>/bin/mono` exists. A Mono
//! not installed at `/usr` runs with brief 0003's environment ([`MonoInstall::env`]).
//!
//! `eludite-dbg-mono` (debuggers/mono, built by `dotnet build dotnet/Eludite.slnx`), [`MonoAdapterSearch`]: beside the
//! Eludite executable (`<exe dir>/eludite-dbg-mono/eludite-dbg-mono.exe`, then `<exe dir>/eludite-dbg-mono.exe`), then
//! the setting `debugger.monoAdapterPath` (`ELUDITE_DBG_MONO`, the file or its folder). It runs as
//! `mono eludite-dbg-mono.exe` ([`MonoInstall::adapter_transport`]).

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::AdapterTransport;

/// The environment variable naming netcoredbg.
pub const ENV_VAR: &str = "ELUDITE_NETCOREDBG";

/// netcoredbg's executable name on this OS.
pub fn netcoredbg_name() -> &'static str {
    if cfg!(windows) {
        "netcoredbg.exe"
    } else {
        "netcoredbg"
    }
}

/// Where netcoredbg was found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FoundIn {
    Bundled,
    Env,
    Path,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Found {
    pub path: PathBuf,
    pub source: FoundIn,
}

impl Found {
    /// The stdio transport that runs it as a DAP server (`--interpreter=vscode`).
    pub fn transport(&self) -> AdapterTransport {
        AdapterTransport::Stdio {
            command: self.path.to_string_lossy().into_owned(),
            args: vec!["--interpreter=vscode".into()],
        }
    }
}

/// The inputs of the search, so tests do not depend on the machine.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AdapterSearch {
    pub exe_dir: Option<PathBuf>,
    pub env: Option<OsString>,
    pub path: Option<OsString>,
}

impl AdapterSearch {
    pub fn from_env() -> Self {
        Self {
            exe_dir: std::env::current_exe()
                .ok()
                .and_then(|p| p.parent().map(Path::to_path_buf)),
            env: std::env::var_os(ENV_VAR).filter(|v| !v.is_empty()),
            path: std::env::var_os("PATH"),
        }
    }

    /// Find netcoredbg, or say where it was looked for.
    pub fn find_netcoredbg(&self) -> Result<Found, String> {
        let name = netcoredbg_name();
        let is_file = |p: &Path| p.is_file();
        if let Some(dir) = &self.exe_dir {
            for p in [dir.join("netcoredbg").join(name), dir.join(name)] {
                if is_file(&p) {
                    return Ok(Found {
                        path: p,
                        source: FoundIn::Bundled,
                    });
                }
            }
        }
        if let Some(env) = &self.env {
            let p = PathBuf::from(env);
            let candidate = if p.is_dir() { p.join(name) } else { p };
            if is_file(&candidate) {
                return Ok(Found {
                    path: candidate,
                    source: FoundIn::Env,
                });
            }
            return Err(format!(
                "{ENV_VAR} is set to {}, which is not netcoredbg",
                candidate.display()
            ));
        }
        if let Some(path) = &self.path {
            for dir in std::env::split_paths(path) {
                let p = dir.join(name);
                if is_file(&p) {
                    return Ok(Found {
                        path: p,
                        source: FoundIn::Path,
                    });
                }
            }
        }
        Err(format!(
            "netcoredbg was not found beside Eludite, in {ENV_VAR} or on PATH; run tools/netcoredbg/fetch.sh and set \
             {ENV_VAR} to the path it prints"
        ))
    }
}

/// The environment variable naming Mono's prefix (the setting `debugger.monoPrefix`).
pub const MONO_PREFIX_ENV: &str = "ELUDITE_MONO_PREFIX";
/// The environment variable naming `eludite-dbg-mono.exe` (the setting `debugger.monoAdapterPath`).
pub const MONO_ADAPTER_ENV: &str = "ELUDITE_DBG_MONO";
/// The Mono adapter's file name.
pub const MONO_ADAPTER: &str = "eludite-dbg-mono.exe";
/// Where `dotnet build dotnet/Eludite.slnx` writes the Mono adapter, from the repository root.
pub const MONO_ADAPTER_BUILT: &str =
    "debuggers/mono/Eludite.Debugger.Mono/bin/Debug/net472/eludite-dbg-mono.exe";

/// The system prefixes, searched last, in order (after `~/.local/opt/mono-root/usr`).
pub const MONO_SYSTEM_PREFIXES: [&str; 3] = [
    "/usr",
    "/usr/local",
    "/Library/Frameworks/Mono.framework/Versions/Current",
];

fn mono_name() -> &'static str {
    if cfg!(windows) { "mono.exe" } else { "mono" }
}

/// Where Mono was found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MonoSource {
    /// The setting `debugger.monoPrefix` (or `ELUDITE_MONO_PREFIX`).
    Configured,
    /// `mono` on `PATH`.
    Path,
    /// `~/.local/opt/mono-root/usr` (brief 0003's user-space Mono).
    UserSpace,
    /// `/usr`, `/usr/local` or the macOS framework.
    System,
}

/// A located Mono.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MonoInstall {
    pub prefix: PathBuf,
    /// `<prefix>/bin/mono`.
    pub mono: PathBuf,
    pub source: MonoSource,
    /// Variables to set when running it: `PATH` with `<prefix>/bin` first, and for a prefix other than `/usr`
    /// `LD_LIBRARY_PATH`, `MONO_CFG_DIR` and `MONO_GAC_PREFIX` (brief 0003's relocated Mono).
    pub env: Vec<(String, String)>,
}

impl MonoInstall {
    /// The stdio transport that runs `adapter` (`eludite-dbg-mono.exe`) under this Mono.
    pub fn adapter_transport(&self, adapter: &Path) -> AdapterTransport {
        AdapterTransport::Stdio {
            command: self.mono.to_string_lossy().into_owned(),
            args: vec![adapter.to_string_lossy().into_owned()],
        }
    }

    /// The version from the first line of `mono --version` (`6.8.0.105`). Runs Mono: call it off the UI thread.
    pub fn version(&self) -> Option<String> {
        let out = Command::new(&self.mono)
            .arg("--version")
            .envs(self.env.iter().map(|(k, v)| (k, v)))
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output()
            .ok()?;
        parse_mono_version(&String::from_utf8_lossy(&out.stdout))
    }
}

/// The version in `mono --version`'s first line: `Mono JIT compiler version 6.8.0.105 (Debian ...)` gives `6.8.0.105`.
pub fn parse_mono_version(output: &str) -> Option<String> {
    let first = output.lines().next()?;
    let after = &first[first.find("version ")? + "version ".len()..];
    let v = after.split_whitespace().next()?;
    (!v.is_empty()).then(|| v.to_owned())
}

/// The variables a Mono at `prefix` runs with, given the current `PATH` and `LD_LIBRARY_PATH`.
pub fn mono_env(
    prefix: &Path,
    path: Option<&OsString>,
    ld: Option<&OsString>,
) -> Vec<(String, String)> {
    let bin = prefix.join("bin");
    let sep = if cfg!(windows) { ";" } else { ":" };
    let current = path
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_default();
    let has_bin = std::env::split_paths(&OsString::from(&current)).any(|d| d == bin);
    let path = if has_bin {
        current
    } else if current.is_empty() {
        bin.to_string_lossy().into_owned()
    } else {
        format!("{}{sep}{current}", bin.display())
    };
    let mut env = vec![("PATH".to_owned(), path)];
    let normalized = prefix.to_string_lossy().trim_end_matches('/').to_owned();
    if normalized != "/usr" {
        let lib = prefix.join("lib").to_string_lossy().into_owned();
        let ld = ld
            .map(|l| l.to_string_lossy().into_owned())
            .filter(|l| !l.is_empty());
        env.push((
            "LD_LIBRARY_PATH".to_owned(),
            match ld {
                Some(l) => format!("{lib}{sep}{l}"),
                None => lib,
            },
        ));
        let etc = prefix
            .parent()
            .map(|p| p.join("etc"))
            .unwrap_or_else(|| prefix.join("etc"));
        env.push((
            "MONO_CFG_DIR".to_owned(),
            etc.to_string_lossy().into_owned(),
        ));
        env.push((
            "MONO_GAC_PREFIX".to_owned(),
            prefix.to_string_lossy().into_owned(),
        ));
    }
    env
}

/// The inputs of the Mono search, so tests do not depend on the machine.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MonoSearch {
    /// The setting `debugger.monoPrefix` (the store resolves `ELUDITE_MONO_PREFIX`).
    pub configured: Option<PathBuf>,
    pub path: Option<OsString>,
    pub home: Option<PathBuf>,
    pub ld_library_path: Option<OsString>,
    /// The system prefixes (`/usr`, `/usr/local`, the macOS framework); tests replace them.
    pub system: Vec<PathBuf>,
}

impl MonoSearch {
    /// The machine's `PATH`, home and system prefixes, without a configured prefix (the settings store gives that).
    pub fn from_env() -> Self {
        Self {
            configured: None,
            path: std::env::var_os("PATH"),
            home: std::env::var_os("HOME")
                .or_else(|| std::env::var_os("USERPROFILE"))
                .map(PathBuf::from),
            ld_library_path: std::env::var_os("LD_LIBRARY_PATH"),
            system: MONO_SYSTEM_PREFIXES.iter().map(PathBuf::from).collect(),
        }
    }

    /// The prefixes to try, in order, with where each came from.
    fn candidates(&self) -> Vec<(PathBuf, MonoSource)> {
        let mut c = Vec::new();
        if let Some(p) = &self.configured {
            c.push((p.clone(), MonoSource::Configured));
        }
        if let Some(path) = &self.path {
            for dir in std::env::split_paths(path) {
                let exe = dir.join(mono_name());
                if exe.is_file() {
                    let real = std::fs::canonicalize(&exe).unwrap_or(exe);
                    if let Some(prefix) = real.parent().and_then(Path::parent) {
                        c.push((prefix.to_path_buf(), MonoSource::Path));
                    }
                }
            }
        }
        if let Some(home) = &self.home {
            c.push((home.join(".local/opt/mono-root/usr"), MonoSource::UserSpace));
        }
        c.extend(self.system.iter().map(|p| (p.clone(), MonoSource::System)));
        c
    }

    /// Find Mono, or say where it was looked for and how to install it.
    pub fn find_mono(&self) -> Result<MonoInstall, String> {
        for (prefix, source) in self.candidates() {
            let mono = prefix.join("bin").join(mono_name());
            if mono.is_file() {
                let env = mono_env(&prefix, self.path.as_ref(), self.ld_library_path.as_ref());
                return Ok(MonoInstall {
                    prefix,
                    mono,
                    source,
                    env,
                });
            }
        }
        let configured = self
            .configured
            .as_ref()
            .map(|p| {
                format!(
                    "debugger.monoPrefix ({}, which has no bin/mono), ",
                    p.display()
                )
            })
            .unwrap_or_else(|| format!("debugger.monoPrefix ({MONO_PREFIX_ENV}, not set), "));
        Err(format!(
            "Mono was not found: searched {configured}mono on PATH, ~/.local/opt/mono-root/usr, {}. Install it \
             (mono-devel on Debian and Ubuntu, mono on Arch, the Mono framework on macOS) or set debugger.monoPrefix \
             to its prefix.",
            self.system
                .iter()
                .map(|p| p.display().to_string())
                .collect::<Vec<_>>()
                .join(", ")
        ))
    }
}

/// The inputs of the `eludite-dbg-mono` search.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MonoAdapterSearch {
    pub exe_dir: Option<PathBuf>,
    /// The setting `debugger.monoAdapterPath` (the store resolves `ELUDITE_DBG_MONO`): the file or its folder.
    pub configured: Option<PathBuf>,
}

impl MonoAdapterSearch {
    pub fn from_env() -> Self {
        Self {
            exe_dir: std::env::current_exe()
                .ok()
                .and_then(|p| p.parent().map(Path::to_path_buf)),
            configured: None,
        }
    }

    /// Find `eludite-dbg-mono.exe`, or name both places and how to build it.
    pub fn find(&self) -> Result<PathBuf, String> {
        let beside = self.exe_dir.as_ref().map(|dir| {
            [
                dir.join("eludite-dbg-mono").join(MONO_ADAPTER),
                dir.join(MONO_ADAPTER),
            ]
        });
        if let Some(found) = beside.iter().flatten().find(|p| p.is_file()) {
            return Ok(found.clone());
        }
        if let Some(c) = &self.configured {
            let candidate = if c.is_dir() {
                c.join(MONO_ADAPTER)
            } else {
                c.clone()
            };
            if candidate.is_file() {
                return Ok(candidate);
            }
            return Err(format!(
                "debugger.monoAdapterPath ({MONO_ADAPTER_ENV}) is {}, which is not {MONO_ADAPTER}; build it with \
                 `dotnet build dotnet/Eludite.slnx` (it writes {MONO_ADAPTER_BUILT})",
                candidate.display()
            ));
        }
        let beside = beside
            .map(|[a, b]| format!("{} or {}", a.display(), b.display()))
            .unwrap_or_else(|| "beside the eludite executable".to_owned());
        Err(format!(
            "{MONO_ADAPTER} was not found beside Eludite ({beside}) or at the setting debugger.monoAdapterPath \
             ({MONO_ADAPTER_ENV}); build it with `dotnet build dotnet/Eludite.slnx`, which writes {MONO_ADAPTER_BUILT}, \
             and set debugger.monoAdapterPath to that file"
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn touch(p: &Path) {
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, b"").unwrap();
    }

    #[test]
    fn search_order_is_bundled_env_path() {
        let t = tempfile::tempdir().unwrap();
        let name = netcoredbg_name();
        let exe = t.path().join("bin");
        let env_dir = t.path().join("env");
        let on_path = t.path().join("path");
        touch(&env_dir.join(name));
        touch(&on_path.join(name));
        let path = std::env::join_paths([t.path().join("nothing"), on_path.clone()]).unwrap();
        let mut s = AdapterSearch {
            exe_dir: Some(exe.clone()),
            env: None,
            path: Some(path),
        };
        assert_eq!(s.find_netcoredbg().unwrap().source, FoundIn::Path);
        // A folder in the variable means the executable inside it.
        s.env = Some(env_dir.clone().into());
        let found = s.find_netcoredbg().unwrap();
        assert_eq!(
            (found.source, found.path),
            (FoundIn::Env, env_dir.join(name))
        );
        touch(&exe.join("netcoredbg").join(name));
        let found = s.find_netcoredbg().unwrap();
        assert_eq!(found.source, FoundIn::Bundled);
        assert_eq!(found.path, exe.join("netcoredbg").join(name));
        assert_eq!(
            found.transport(),
            AdapterTransport::Stdio {
                command: found.path.to_string_lossy().into_owned(),
                args: vec!["--interpreter=vscode".into()]
            }
        );
        // A wrong variable is an error, not a silent fallback to PATH.
        let wrong = AdapterSearch {
            exe_dir: None,
            env: Some(t.path().join("missing").into()),
            path: s.path.clone(),
        };
        assert!(wrong.find_netcoredbg().unwrap_err().contains(ENV_VAR));
        let none = AdapterSearch::default();
        assert!(none.find_netcoredbg().unwrap_err().contains("fetch.sh"));
    }

    #[test]
    fn mono_search_order_is_setting_path_user_space_system() {
        let t = tempfile::tempdir().unwrap();
        let mk = |p: &Path| touch(&p.join("bin").join(mono_name()));
        let configured = t.path().join("configured");
        let on_path = t.path().join("onpath");
        let home = t.path().join("home");
        let system = t.path().join("sys");
        mk(&system);
        let path = std::env::join_paths([t.path().join("nothing"), on_path.join("bin")]).unwrap();
        let mut s = MonoSearch {
            configured: None,
            path: Some(path),
            home: Some(home.clone()),
            ld_library_path: None,
            system: vec![t.path().join("nosys"), system.clone()],
        };
        let found = s.find_mono().unwrap();
        assert_eq!(
            (found.source, found.prefix.clone()),
            (MonoSource::System, system.clone())
        );
        let user = home.join(".local/opt/mono-root/usr");
        mk(&user);
        assert_eq!(s.find_mono().unwrap().source, MonoSource::UserSpace);
        mk(&on_path);
        let found = s.find_mono().unwrap();
        assert_eq!(found.source, MonoSource::Path);
        // Two folders up from the resolved executable.
        assert_eq!(found.prefix, std::fs::canonicalize(&on_path).unwrap());
        // A configured prefix without bin/mono falls through; with one it wins.
        s.configured = Some(configured.clone());
        assert_eq!(s.find_mono().unwrap().source, MonoSource::Path);
        mk(&configured);
        let found = s.find_mono().unwrap();
        assert_eq!(
            (found.source, found.mono.clone()),
            (
                MonoSource::Configured,
                configured.join("bin").join(mono_name())
            )
        );
        // A relocated prefix runs with brief 0003's environment.
        let env: std::collections::HashMap<_, _> = found.env.iter().cloned().collect();
        assert!(env["PATH"].starts_with(&configured.join("bin").to_string_lossy().into_owned()));
        assert_eq!(
            env["LD_LIBRARY_PATH"],
            configured.join("lib").to_string_lossy()
        );
        assert_eq!(env["MONO_GAC_PREFIX"], configured.to_string_lossy());
        assert_eq!(env["MONO_CFG_DIR"], t.path().join("etc").to_string_lossy());
        assert_eq!(
            found.adapter_transport(Path::new("/a/eludite-dbg-mono.exe")),
            AdapterTransport::Stdio {
                command: found.mono.to_string_lossy().into_owned(),
                args: vec!["/a/eludite-dbg-mono.exe".into()]
            }
        );
        // /usr needs only PATH.
        let usr = mono_env(
            Path::new("/usr"),
            Some(&OsString::from("/usr/bin:/bin")),
            None,
        );
        assert_eq!(usr, vec![("PATH".to_owned(), "/usr/bin:/bin".to_owned())]);

        let none = MonoSearch::default();
        let err = none.find_mono().unwrap_err();
        assert!(
            err.contains("mono-devel") && err.contains("debugger.monoPrefix"),
            "{err}"
        );
        assert_eq!(
            parse_mono_version(
                "Mono JIT compiler version 6.8.0.105 (Debian 6.8.0.105+dfsg-3.6 Wed Jul 31 2024)\nCopyright"
            )
            .as_deref(),
            Some("6.8.0.105")
        );
        assert_eq!(parse_mono_version("nonsense"), None);
    }

    #[test]
    fn mono_adapter_search_is_beside_then_setting() {
        let t = tempfile::tempdir().unwrap();
        let exe = t.path().join("bin");
        let built = t.path().join("built");
        touch(&built.join(MONO_ADAPTER));
        let mut s = MonoAdapterSearch {
            exe_dir: Some(exe.clone()),
            configured: None,
        };
        let err = s.find().unwrap_err();
        assert!(
            err.contains(
                &exe.join("eludite-dbg-mono")
                    .join(MONO_ADAPTER)
                    .display()
                    .to_string()
            ) && err.contains(MONO_ADAPTER_ENV)
                && err.contains("dotnet build dotnet/Eludite.slnx")
                && err.contains(MONO_ADAPTER_BUILT),
            "{err}"
        );
        // The setting: a folder means the adapter inside it.
        s.configured = Some(built.clone());
        assert_eq!(s.find().unwrap(), built.join(MONO_ADAPTER));
        s.configured = Some(t.path().join("missing.exe"));
        assert!(s.find().unwrap_err().contains("missing.exe"));
        // Beside the executable wins: the folder first, then the file.
        touch(&exe.join(MONO_ADAPTER));
        assert_eq!(s.find().unwrap(), exe.join(MONO_ADAPTER));
        touch(&exe.join("eludite-dbg-mono").join(MONO_ADAPTER));
        assert_eq!(
            s.find().unwrap(),
            exe.join("eludite-dbg-mono").join(MONO_ADAPTER)
        );
    }
}
