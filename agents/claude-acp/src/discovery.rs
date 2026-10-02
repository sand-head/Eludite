//! Finding the user's `claude` executable and checking its version.

use std::ffi::OsString;
use std::fmt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// The `claude` version this adapter was validated against (brief 0006). Older
/// versions are refused rather than guessed at; newer ones run, with a note in
/// the log, because the stream-json protocol is additive in practice.
pub const MIN_CLAUDE_VERSION: Version = Version(2, 1, 287);

/// Environment variable naming the `claude` executable explicitly.
pub const CLAUDE_PATH_ENV: &str = "ELUDITE_CLAUDE_PATH";

/// Variables Claude Code sets for its own child processes. If the adapter (or
/// the IDE that launched it) was started from a Claude Code terminal they would
/// leak into the child and make it believe it is nested inside another session
/// (brief 0005 report, section 3). They are removed from the child's
/// environment; nothing else is touched, so the child inherits the user's login.
pub const CLAUDE_SESSION_ENV: &[&str] = &[
    "CLAUDECODE",
    "CLAUDE_CODE_ENTRYPOINT",
    "CLAUDE_CODE_SESSION_ID",
    "CLAUDE_CODE_CHILD_SESSION",
    "CLAUDE_CODE_BRIDGE_SESSION_ID",
    "CLAUDE_CODE_MESSAGING_SOCKET",
    "CLAUDE_CODE_MESSAGING_TOKEN",
    "CLAUDE_CODE_SESSION_ATTENDED",
    "CLAUDE_CODE_EXECPATH",
    "CLAUDE_PID",
    "CLAUDE_EFFORT",
];

/// The executable's file name on this platform.
pub fn exe_name() -> &'static str {
    if cfg!(windows) {
        "claude.exe"
    } else {
        "claude"
    }
}

/// A `major.minor.patch` version.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Version(pub u32, pub u32, pub u32);

impl Version {
    /// Parse the first `x.y.z` in `text`, e.g. `2.1.287 (Claude Code)`.
    pub fn parse(text: &str) -> Option<Self> {
        let word = text.split_whitespace().next()?;
        let mut parts = word.split('.').map(|p| p.parse::<u32>().ok());
        let v = Version(parts.next()??, parts.next()??, parts.next()??);
        parts.next().is_none().then_some(v)
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.0, self.1, self.2)
    }
}

/// Where the executable was found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    Explicit,
    Env,
    Path,
    LocalBin,
}

/// Find `claude`: an explicit path (the `--claude` argument), then
/// `$ELUDITE_CLAUDE_PATH`, then `$PATH`, then `~/.local/bin` (the native
/// installer's location). `env` and `home` are passed in so tests can control
/// them.
pub fn discover(
    explicit: Option<&Path>,
    env_path: Option<OsString>,
    path_var: Option<OsString>,
    home: Option<PathBuf>,
) -> Result<(PathBuf, Source), String> {
    // The child runs in the session's cwd, so a relative path must be made
    // absolute against the adapter's own.
    find(explicit, env_path, path_var, home).map(|(p, s)| (std::path::absolute(&p).unwrap_or(p), s))
}

fn find(
    explicit: Option<&Path>,
    env_path: Option<OsString>,
    path_var: Option<OsString>,
    home: Option<PathBuf>,
) -> Result<(PathBuf, Source), String> {
    if let Some(p) = explicit {
        return if p.is_file() {
            Ok((p.to_owned(), Source::Explicit))
        } else {
            Err(format!("--claude {}: no such file", p.display()))
        };
    }
    if let Some(p) = env_path.filter(|p| !p.is_empty()) {
        let p = PathBuf::from(p);
        return if p.is_file() {
            Ok((p, Source::Env))
        } else {
            Err(format!("{CLAUDE_PATH_ENV}={}: no such file", p.display()))
        };
    }
    if let Some(path_var) = path_var {
        for dir in std::env::split_paths(&path_var) {
            let candidate = dir.join(exe_name());
            if is_executable(&candidate) {
                return Ok((candidate, Source::Path));
            }
        }
    }
    if let Some(home) = home {
        let candidate = home.join(".local").join("bin").join(exe_name());
        if is_executable(&candidate) {
            return Ok((candidate, Source::LocalBin));
        }
    }
    Err(format!(
        "Claude Code (`{}`) was not found on PATH or in ~/.local/bin. Install it from \
         https://claude.com/claude-code, or pass --claude PATH or set {CLAUDE_PATH_ENV}.",
        exe_name()
    ))
}

/// Discover using the process environment.
pub fn discover_from_env(explicit: Option<&Path>) -> Result<(PathBuf, Source), String> {
    let home =
        std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).map(PathBuf::from);
    discover(
        explicit,
        std::env::var_os(CLAUDE_PATH_ENV),
        std::env::var_os("PATH"),
        home,
    )
}

fn is_executable(p: &Path) -> bool {
    let Ok(meta) = p.metadata() else {
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

/// Run `claude --version` (with Claude Code's session variables removed) and
/// parse it.
pub fn version_of(claude: &Path) -> Result<Version, String> {
    let mut cmd = Command::new(claude);
    cmd.arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for var in CLAUDE_SESSION_ENV {
        cmd.env_remove(var);
    }
    let out = cmd
        .output()
        .map_err(|e| format!("could not run `{} --version`: {e}", claude.display()))?;
    let text = String::from_utf8_lossy(&out.stdout);
    Version::parse(text.trim()).ok_or_else(|| {
        format!(
            "`{} --version` printed {:?}, not a version",
            claude.display(),
            text.trim()
        )
    })
}

/// Refuse versions older than [`MIN_CLAUDE_VERSION`].
pub fn check_version(v: Version) -> Result<(), String> {
    if v < MIN_CLAUDE_VERSION {
        Err(format!(
            "Claude Code {v} is older than {MIN_CLAUDE_VERSION}, the oldest version \
             eludite-claude-acp is validated against. Update it (`claude update`) and start a \
             new session."
        ))
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_cli_version_output() {
        assert_eq!(
            Version::parse("2.1.287 (Claude Code)"),
            Some(Version(2, 1, 287))
        );
        assert_eq!(Version::parse("10.0.1"), Some(Version(10, 0, 1)));
        assert_eq!(Version::parse("2.1"), None);
        assert_eq!(Version::parse("2.1.x"), None);
        assert_eq!(Version::parse(""), None);
    }

    #[test]
    fn refuses_older_versions_only() {
        assert!(check_version(Version(2, 1, 286)).is_err());
        assert!(check_version(Version(1, 9, 999)).is_err());
        assert!(check_version(MIN_CLAUDE_VERSION).is_ok());
        assert!(check_version(Version(2, 2, 0)).is_ok());
        let msg = check_version(Version(2, 0, 1)).unwrap_err();
        assert!(msg.contains("2.0.1") && msg.contains("2.1.287"), "{msg}");
    }

    fn temp_dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "eludite-claude-acp-disc-{}-{name}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn make_exe(p: &Path) {
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, "#!/bin/sh\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
    }

    #[test]
    fn discovery_order() {
        let root = temp_dir("order");
        let on_path = root.join("pathdir").join(exe_name());
        let local = root
            .join("home")
            .join(".local")
            .join("bin")
            .join(exe_name());
        let explicit = root.join("explicit").join(exe_name());
        make_exe(&on_path);
        make_exe(&local);
        make_exe(&explicit);
        let path_var = std::env::join_paths([root.join("empty"), root.join("pathdir")]).unwrap();
        let home = Some(root.join("home"));

        let (p, s) = discover(
            Some(&explicit),
            Some(on_path.clone().into()),
            Some(path_var.clone()),
            home.clone(),
        )
        .unwrap();
        assert_eq!((p, s), (explicit.clone(), Source::Explicit));

        let (p, s) = discover(
            None,
            Some(local.clone().into()),
            Some(path_var.clone()),
            home.clone(),
        )
        .unwrap();
        assert_eq!((p, s), (local.clone(), Source::Env));

        let (p, s) = discover(None, None, Some(path_var), home.clone()).unwrap();
        assert_eq!((p, s), (on_path, Source::Path));

        let (p, s) = discover(None, None, Some(root.join("empty").into()), home).unwrap();
        assert_eq!((p, s), (local, Source::LocalBin));

        let err = discover(None, None, Some(root.join("empty").into()), None).unwrap_err();
        assert!(err.contains("not found"), "{err}");
        assert!(discover(Some(&root.join("nope")), None, None, None).is_err());
        let (rel, _) = discover(Some(Path::new("Cargo.toml")), None, None, None).unwrap();
        assert!(rel.is_absolute(), "{rel:?}");
        let _ = std::fs::remove_dir_all(&root);
    }
}
