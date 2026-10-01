//! Locating netcoredbg (MIT, https://github.com/Samsung/netcoredbg), which is located on the machine, never vendored
//! into git (`tools/netcoredbg/fetch.sh` downloads a pinned release). The search order:
//!
//! 1. bundled beside the Eludite executable: `<exe dir>/netcoredbg/netcoredbg` (the release archive's folder), then
//!    `<exe dir>/netcoredbg`;
//! 2. `ELUDITE_NETCOREDBG`: the executable, or the folder holding it;
//! 3. `PATH`.
//!
//! The executable is `netcoredbg.exe` on Windows.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

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
}
