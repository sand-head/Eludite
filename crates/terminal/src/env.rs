//! The terminal's environment: the folders of the tools Eludite located (the `dotnet` it builds with, Cargo, Node.js)
//! put first on PATH once each, as Visual Studio's Developer PowerShell puts its own tools first, and the variables
//! that say the terminal is Eludite's (`TERM_PROGRAM=Eludite`, `TERM=xterm-256color`, `COLORTERM=truecolor`).

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

use crate::profile::which;

/// The located tools' folders, in the order they go on PATH, and variables to set with them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ToolPaths {
    pub dirs: Vec<PathBuf>,
    pub vars: BTreeMap<String, String>,
}

impl ToolPaths {
    /// Locate the tools from Eludite's own environment `env`: `dotnet` (`DOTNET_ROOT`, else PATH, symlinks
    /// resolved; `DOTNET_ROOT` is set to its folder), Cargo (`cargo` given, else `CARGO_HOME/bin`, `~/.cargo/bin`,
    /// else PATH), Node.js (`ELUDITE_NODE`, else PATH). Missing tools are skipped.
    pub fn locate(env: &dyn Fn(&str) -> Option<String>, cargo: Option<&Path>) -> Self {
        let path = env("PATH");
        let mut out = Self::default();
        let dotnet = env("DOTNET_ROOT")
            .map(|r| Path::new(&r).join(exe("dotnet")))
            .filter(|p| p.is_file())
            .or_else(|| which("dotnet", path.as_deref()))
            .map(|p| std::fs::canonicalize(&p).unwrap_or(p));
        if let Some(dir) = dotnet.as_deref().and_then(Path::parent) {
            out.vars
                .insert("DOTNET_ROOT".into(), dir.to_string_lossy().into_owned());
            out.push(dir);
        }
        let home = env("HOME").or_else(|| env("USERPROFILE"));
        let cargo = cargo
            .map(Path::to_path_buf)
            .filter(|p| p.is_file())
            .or_else(|| {
                env("CARGO_HOME")
                    .map(PathBuf::from)
                    .or_else(|| home.as_ref().map(|h| Path::new(h).join(".cargo")))
                    .map(|c| c.join("bin").join(exe("cargo")))
                    .filter(|p| p.is_file())
            })
            .or_else(|| which("cargo", path.as_deref()));
        if let Some(dir) = cargo.as_deref().and_then(Path::parent) {
            out.push(dir);
        }
        let node = env("ELUDITE_NODE")
            .map(PathBuf::from)
            .filter(|p| p.is_file())
            .or_else(|| which("node", path.as_deref()));
        if let Some(dir) = node.as_deref().and_then(Path::parent) {
            out.push(dir);
        }
        out
    }

    fn push(&mut self, dir: &Path) {
        if !self.dirs.iter().any(|d| d == dir) {
            self.dirs.push(dir.to_path_buf());
        }
    }
}

fn exe(name: &str) -> String {
    if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.to_owned()
    }
}

/// `path` (a PATH value) with `dirs` first, each once: an entry equal to one of them further on is removed.
pub fn prepend_path(path: Option<&str>, dirs: &[PathBuf]) -> OsString {
    let mut entries: Vec<PathBuf> = Vec::new();
    for d in dirs {
        if !entries.contains(d) {
            entries.push(d.clone());
        }
    }
    for e in path
        .map(|p| std::env::split_paths(p).collect::<Vec<_>>())
        .unwrap_or_default()
    {
        if !entries.contains(&e) {
            entries.push(e);
        }
    }
    std::env::join_paths(entries).unwrap_or_default()
}

/// The variables every terminal gets, then `tools` (when `inherit` is on) and the profile's and the caller's.
pub fn terminal_env(
    base_path: Option<&str>,
    tools: Option<&ToolPaths>,
    extra: &BTreeMap<String, String>,
) -> BTreeMap<String, String> {
    let mut env = BTreeMap::new();
    env.insert("TERM".into(), "xterm-256color".into());
    env.insert("COLORTERM".into(), "truecolor".into());
    env.insert("TERM_PROGRAM".into(), "Eludite".into());
    env.insert(
        "TERM_PROGRAM_VERSION".into(),
        env!("CARGO_PKG_VERSION").into(),
    );
    if let Some(t) = tools {
        env.insert(
            "PATH".into(),
            prepend_path(base_path, &t.dirs)
                .to_string_lossy()
                .into_owned(),
        );
        for (k, v) in &t.vars {
            env.insert(k.clone(), v.clone());
        }
    }
    for (k, v) in extra {
        env.insert(k.clone(), v.clone());
    }
    env
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_folders_go_first_once() {
        let sep = if cfg!(windows) { ";" } else { ":" };
        let base = ["/usr/bin", "/opt/dotnet", "/bin"].join(sep);
        let dirs = vec![
            PathBuf::from("/opt/dotnet"),
            PathBuf::from("/home/me/.cargo/bin"),
        ];
        let out = prepend_path(Some(&base), &dirs);
        let parts: Vec<PathBuf> = std::env::split_paths(&out).collect();
        assert_eq!(
            parts,
            ["/opt/dotnet", "/home/me/.cargo/bin", "/usr/bin", "/bin"].map(PathBuf::from)
        );
    }

    #[test]
    fn locate_finds_dotnet_by_its_root_and_sets_it() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("dotnet");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join(exe("dotnet")), "").unwrap();
        let cargo_bin = dir.path().join("cargo").join("bin");
        std::fs::create_dir_all(&cargo_bin).unwrap();
        std::fs::write(cargo_bin.join(exe("cargo")), "").unwrap();
        let root_s = root.to_string_lossy().into_owned();
        let cargo_home = dir.path().join("cargo").to_string_lossy().into_owned();
        let env = move |k: &str| match k {
            "DOTNET_ROOT" => Some(root_s.clone()),
            "CARGO_HOME" => Some(cargo_home.clone()),
            _ => None,
        };
        let t = ToolPaths::locate(&env, None);
        let real = std::fs::canonicalize(&root).unwrap();
        assert_eq!(t.dirs[0], real);
        assert_eq!(t.dirs[1], cargo_bin);
        assert_eq!(t.vars["DOTNET_ROOT"], real.to_string_lossy());
        let e = terminal_env(Some("/usr/bin"), Some(&t), &BTreeMap::new());
        assert!(e["PATH"].starts_with(&real.to_string_lossy().into_owned()));
        assert_eq!(e["TERM_PROGRAM"], "Eludite");
    }
}
