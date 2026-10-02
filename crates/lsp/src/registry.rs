//! Language-server registration as data (brief 0019): which server handles which files, how to find and start it,
//! and what to send it. The built-in registrations are `servers.json` beside this file; the shell has no
//! per-language code path, so adding a language is adding an entry.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde::{Deserialize, Serialize};
use serde_json::Value;

const BUILTIN: &str = include_str!("servers.json");

/// How a server is reached.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Via {
    /// Behind `eludite-host` (Roslyn): the host bridge.
    EluditeHost,
    /// A process the shell launches and speaks plain LSP to: the generic client.
    Process,
}

/// How to find a server's executable.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CommandSpec {
    /// The executable's name without the platform suffix (`rust-analyzer`; `.exe` is added on Windows).
    pub executable: String,
    #[serde(default)]
    pub args: Vec<String>,
    /// An environment variable naming the executable (`ELUDITE_RUST_ANALYZER`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub env_override: Option<String>,
    /// A rustup component that provides it (`rust-analyzer`), found with `rustup which`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rustup_component: Option<String>,
}

/// One language server.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ServerRegistration {
    /// Stable id (`rust-analyzer`, `roslyn`).
    pub id: String,
    /// The status bar's name for it (`rust-analyzer`, `C#`).
    pub name: String,
    /// The LSP `languageId` of its documents.
    pub language_id: String,
    /// File-name patterns it handles (`*.rs`); `*` matches any run of characters.
    pub file_globs: Vec<String>,
    pub via: Via,
    /// Files whose folder is a workspace root for it (`Cargo.toml`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub root_markers: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<CommandSpec>,
    /// LSP `initializationOptions`.
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub initialization_options: Value,
    /// The answers to `workspace/configuration`, by section.
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub settings: Value,
}

/// Where a server's executable was found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Located {
    pub path: PathBuf,
    /// The first line of its `--version` output.
    pub version: String,
    /// `beside eludite`, `ELUDITE_RUST_ANALYZER`, `PATH`, `rustup`.
    pub source: String,
}

/// What [`ServerRegistration::locate_with`] may consult; the real process environment in
/// [`ServerRegistration::locate`].
pub struct Environment<'a> {
    /// The folder of the `eludite` executable.
    pub beside: Option<&'a Path>,
    pub var: &'a dyn Fn(&str) -> Option<OsString>,
    pub path_var: Option<OsString>,
    /// `--version` of a candidate: its first line, or `None` when it does not run.
    pub probe: &'a dyn Fn(&Path) -> Option<String>,
    /// `rustup which <executable>`.
    pub rustup: &'a dyn Fn(&str) -> Option<PathBuf>,
}

impl ServerRegistration {
    /// Whether this server handles the file at `path` (by file name).
    pub fn matches(&self, path: &Path) -> bool {
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            return false;
        };
        self.file_globs.iter().any(|g| glob_match(g, name))
    }

    /// The nearest folder at or above `file`'s that holds one of the root markers.
    pub fn find_root(&self, file: &Path) -> Option<PathBuf> {
        let start = if file.is_dir() { file } else { file.parent()? };
        start
            .ancestors()
            .find(|dir| self.root_markers.iter().any(|m| dir.join(m).is_file()))
            .map(Path::to_path_buf)
    }

    /// Find the executable in the real environment: beside the `eludite` executable, then the override variable,
    /// then `PATH`, then the rustup component. Each candidate must answer `--version` (a rustup proxy without the
    /// component does not). Spawns processes: call it off the UI thread.
    pub fn locate(&self) -> Result<Located, String> {
        let beside = std::env::current_exe()
            .ok()
            .and_then(|exe| exe.parent().map(Path::to_path_buf));
        self.locate_with(&Environment {
            beside: beside.as_deref(),
            var: &|k| std::env::var_os(k),
            path_var: std::env::var_os("PATH"),
            probe: &probe_version,
            rustup: &rustup_which,
        })
    }

    /// [`ServerRegistration::locate`] against `env`.
    pub fn locate_with(&self, env: &Environment<'_>) -> Result<Located, String> {
        let Some(cmd) = &self.command else {
            return Err(format!("{} is not launched by the shell", self.id));
        };
        let exe = format!("{}{}", cmd.executable, std::env::consts::EXE_SUFFIX);
        let try_path = |p: &Path, source: &str| -> Option<Located> {
            if !p.is_file() {
                return None;
            }
            (env.probe)(p).map(|version| Located {
                path: p.to_path_buf(),
                version,
                source: source.to_owned(),
            })
        };
        if let Some(dir) = env.beside
            && let Some(found) = try_path(&dir.join(&exe), "beside eludite")
        {
            return Ok(found);
        }
        if let Some(var) = &cmd.env_override
            && let Some(value) = (env.var)(var)
        {
            let p = PathBuf::from(value);
            return try_path(&p, var).ok_or_else(|| format!("{var}={} does not run", p.display()));
        }
        if let Some(paths) = &env.path_var {
            for dir in std::env::split_paths(paths) {
                if let Some(found) = try_path(&dir.join(&exe), "PATH") {
                    return Ok(found);
                }
            }
        }
        if let Some(component) = &cmd.rustup_component
            && let Some(p) = (env.rustup)(&cmd.executable)
            && let Some(found) = try_path(&p, &format!("rustup component {component}"))
        {
            return Ok(found);
        }
        Err(format!(
            "{} not found beside eludite, {}on PATH{}",
            cmd.executable,
            cmd.env_override
                .as_deref()
                .map(|v| format!("in {v}, "))
                .unwrap_or_default(),
            cmd.rustup_component
                .as_deref()
                .map(|c| format!(" or as the rustup component (`rustup component add {c}`)"))
                .unwrap_or_default()
        ))
    }
}

/// The servers Eludite knows.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServerRegistry {
    pub servers: Vec<ServerRegistration>,
}

impl ServerRegistry {
    /// The built-in registrations (`servers.json`).
    pub fn builtin() -> Self {
        Self::from_json(BUILTIN).expect("servers.json is a valid registry")
    }

    pub fn from_json(text: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(text)
    }

    /// The server for the file at `path`, if any.
    pub fn for_path(&self, path: &Path) -> Option<&ServerRegistration> {
        self.servers.iter().find(|s| s.matches(path))
    }

    pub fn get(&self, id: &str) -> Option<&ServerRegistration> {
        self.servers.iter().find(|s| s.id == id)
    }
}

/// `*`-only glob over a file name, ASCII case-insensitive (`*.cs` matches `Program.CS`).
pub fn glob_match(pattern: &str, name: &str) -> bool {
    fn go(p: &[u8], n: &[u8]) -> bool {
        match p.split_first() {
            None => n.is_empty(),
            Some((b'*', rest)) => (0..=n.len()).any(|i| go(rest, &n[i..])),
            Some((c, rest)) => n
                .split_first()
                .is_some_and(|(d, n)| c.eq_ignore_ascii_case(d) && go(rest, n)),
        }
    }
    go(pattern.as_bytes(), name.as_bytes())
}

fn probe_version(path: &Path) -> Option<String> {
    let out = Command::new(path)
        .arg("--version")
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout);
    Some(text.lines().next().unwrap_or_default().trim().to_owned())
}

fn rustup_which(executable: &str) -> Option<PathBuf> {
    let out = Command::new("rustup")
        .args(["which", executable])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    out.status.success().then(|| {
        PathBuf::from(
            String::from_utf8_lossy(&out.stdout)
                .lines()
                .next()
                .unwrap_or_default()
                .trim(),
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_registry_routes_by_file_name() {
        let r = ServerRegistry::builtin();
        let ra = r
            .for_path(Path::new("/w/crates/editor/src/buffer.rs"))
            .unwrap();
        assert_eq!(ra.id, "rust-analyzer");
        assert_eq!(ra.language_id, "rust");
        assert_eq!(ra.via, Via::Process);
        assert_eq!(ra.root_markers, ["Cargo.toml"]);
        assert_eq!(
            ra.command.as_ref().unwrap().env_override.as_deref(),
            Some("ELUDITE_RUST_ANALYZER")
        );
        assert_eq!(
            ra.settings["rust-analyzer"]["cargo"]["targetDir"],
            Value::Bool(true)
        );
        let cs = r.for_path(Path::new("/s/App/Program.cs")).unwrap();
        assert_eq!((cs.id.as_str(), cs.via), ("roslyn", Via::EluditeHost));
        assert!(r.for_path(Path::new("/s/README.md")).is_none());
        assert!(r.for_path(Path::new("/s/Cargo.toml")).is_none());
    }

    #[test]
    fn registrations_are_data() {
        let r = ServerRegistry::from_json(
            r#"{"servers": [{"id": "ts", "name": "TypeScript", "languageId": "typescript",
                "fileGlobs": ["*.ts", "*.tsx"], "via": "process",
                "command": {"executable": "typescript-language-server", "args": ["--stdio"]}}]}"#,
        )
        .unwrap();
        assert_eq!(r.for_path(Path::new("/a/b.tsx")).unwrap().id, "ts");
        assert!(ServerRegistry::from_json(r#"{"servers": [{"id": "x"}]}"#).is_err());
    }

    #[test]
    fn globs() {
        assert!(glob_match("*.rs", "main.rs"));
        assert!(glob_match("*.cs", "Program.CS"));
        assert!(!glob_match("*.rs", "main.rsx"));
        assert!(glob_match("Cargo.toml", "Cargo.toml"));
        assert!(glob_match("*", ""));
        assert!(!glob_match("a*b", "ac"));
    }

    #[test]
    fn roots_come_from_markers() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("crate/src");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(dir.path().join("crate/Cargo.toml"), "").unwrap();
        std::fs::write(src.join("lib.rs"), "").unwrap();
        let ra = ServerRegistry::builtin();
        let ra = ra.get("rust-analyzer").unwrap();
        assert_eq!(
            ra.find_root(&src.join("lib.rs")).as_deref(),
            Some(dir.path().join("crate").as_path())
        );
    }

    #[test]
    fn locate_prefers_beside_then_env_then_path_then_rustup_and_skips_dead_proxies() {
        let dir = tempfile::tempdir().unwrap();
        let exe = format!("rust-analyzer{}", std::env::consts::EXE_SUFFIX);
        let mk = |sub: &str| {
            let d = dir.path().join(sub);
            std::fs::create_dir_all(&d).unwrap();
            std::fs::write(d.join(&exe), "").unwrap();
            d
        };
        let (beside, env_dir, path_dir, proxy_dir, rustup_dir) = (
            mk("beside"),
            mk("env"),
            mk("path"),
            mk("proxy"),
            mk("rustup"),
        );
        let ra = ServerRegistry::builtin();
        let ra = ra.get("rust-analyzer").unwrap();
        // The rustup proxy on PATH answers nothing (no component installed).
        let probe = |p: &Path| {
            (!p.starts_with(dir.path().join("proxy"))).then(|| "rust-analyzer 1.0".to_owned())
        };
        let rustup_path = rustup_dir.join(&exe);
        let rustup = move |_: &str| Some(rustup_path.clone());
        let none = |_: &str| None;
        let env_var = env_dir.join(&exe).into_os_string();
        let with_env = move |k: &str| (k == "ELUDITE_RUST_ANALYZER").then(|| env_var.clone());
        let path_var = std::env::join_paths([&proxy_dir, &path_dir]).ok();
        let env = |beside: Option<&Path>,
                   var: &dyn Fn(&str) -> Option<OsString>,
                   path_var: Option<OsString>| {
            ra.locate_with(&Environment {
                beside,
                var,
                path_var,
                probe: &probe,
                rustup: &rustup,
            })
        };
        let found = env(Some(&beside), &with_env, path_var.clone()).unwrap();
        assert_eq!(
            (found.path.parent().unwrap(), found.source.as_str()),
            (beside.as_path(), "beside eludite")
        );
        let found = env(None, &with_env, path_var.clone()).unwrap();
        assert_eq!(found.source, "ELUDITE_RUST_ANALYZER");
        assert_eq!(found.version, "rust-analyzer 1.0");
        let found = env(None, &none, path_var.clone()).unwrap();
        assert_eq!(
            (found.path.parent().unwrap(), found.source.as_str()),
            (path_dir.as_path(), "PATH")
        );
        let only_proxy = std::env::join_paths([&proxy_dir]).ok();
        let found = env(None, &none, only_proxy).unwrap();
        assert_eq!(found.source, "rustup component rust-analyzer");
        let missing = |_: &str| Some(dir.path().join("nope").into_os_string());
        assert!(
            env(None, &missing, path_var)
                .unwrap_err()
                .contains("does not run")
        );
    }
}
