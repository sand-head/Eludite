//! Finding Node.js (brief 0050, moved here from `eludite-dap`'s discovery, where brief 0038 wrote it): one search for
//! everything Eludite runs on Node, vscode-js-debug (`debugger.nodePath`) and the web language servers and formatters
//! (`languageServers.nodePath`), both overridden by `ELUDITE_NODE`. `eludite-dap` re-exports it; nothing duplicates
//! it. The order: the setting (the executable or its folder), `node` on `PATH`, Volta's `~/.volta/bin/node`, nvm's
//! default (`~/.nvm/alias/default`, the newest installed version it names) and fnm's default. The executable is only
//! looked for here; its version is read with [`node_version_output`] (a process: off the UI thread) and checked with
//! [`check_version`] against what its [`NodeUser`] needs.

use crate::NoConsoleWindow as _;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// The environment variable naming the Node.js executable (the settings `debugger.nodePath` and
/// `languageServers.nodePath`).
pub const NODE_ENV: &str = "ELUDITE_NODE";

/// What runs on the Node.js being looked for, for the messages: the setting that names it, what needs it and the
/// oldest major version that runs it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NodeUser {
    /// `debugger.nodePath`, `languageServers.nodePath`.
    pub setting: &'static str,
    /// `vscode-js-debug`, `the web language servers`.
    pub needs: &'static str,
    pub min_major: u32,
}

/// The web language servers and formatters (brief 0050; `tools/web-servers/PIN`'s `node`).
pub const WEB_SERVERS: NodeUser = NodeUser {
    setting: "languageServers.nodePath",
    needs: "the web language servers",
    min_major: 20,
};

/// Where Node.js was found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeSource {
    /// The setting (or `ELUDITE_NODE`).
    Configured,
    /// `node` on `PATH`.
    Path,
    /// Volta's `~/.volta/bin/node`.
    Volta,
    /// nvm's default version (`~/.nvm/alias/default`).
    Nvm,
    /// fnm's default (`~/.local/share/fnm/aliases/default`, `~/.fnm/aliases/default`).
    Fnm,
}

/// The inputs of the Node.js search, so tests do not depend on the machine.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NodeSearch {
    /// The setting (the store resolves `ELUDITE_NODE`): the executable or its folder.
    pub configured: Option<PathBuf>,
    pub path: Option<OsString>,
    pub home: Option<PathBuf>,
}

/// Node.js's executable name on this OS.
pub fn node_name() -> &'static str {
    if cfg!(windows) { "node.exe" } else { "node" }
}

/// The major version of `node --version`'s output (`v22.12.0` gives 22).
pub fn parse_node_version(output: &str) -> Option<(String, u32)> {
    let v = output.lines().map(str::trim).find(|l| !l.is_empty())?;
    let major = v.trim_start_matches('v').split('.').next()?.parse().ok()?;
    Some((v.to_owned(), major))
}

/// Whether `output` (`node --version`'s) is at least `user`'s minimum: the version, or why not.
pub fn check_version(node: &Path, output: Option<&str>, user: &NodeUser) -> Result<String, String> {
    match output.and_then(parse_node_version) {
        Some((v, major)) if major >= user.min_major => Ok(v),
        Some((v, _)) => Err(format!(
            "Node.js {v} at {} is older than {} runs on (Node.js {} or later); install a newer one or set {}",
            node.display(),
            user.needs,
            user.min_major,
            user.setting
        )),
        None => Err(format!(
            "{} did not say its version (`node --version`); set {} to a Node.js {} or later",
            node.display(),
            user.setting,
            user.min_major
        )),
    }
}

/// The current user's home folder (`HOME`, else `USERPROFILE`).
pub fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .filter(|h| !h.is_empty())
        .map(PathBuf::from)
}

/// A version's numeric parts (`1.140.0` to `[1, 140, 0]`), for ordering.
fn version_key(v: &str) -> Vec<u64> {
    v.trim_start_matches('v')
        .split(['.', '-'])
        .map(|p| p.parse().unwrap_or(0))
        .collect()
}

impl NodeSearch {
    /// The machine's `PATH` and home, without a configured path (the settings store gives that).
    pub fn from_env() -> Self {
        Self {
            configured: None,
            path: std::env::var_os("PATH"),
            home: home_dir(),
        }
    }

    /// nvm's default version's node: `~/.nvm/alias/default` names a version (`v22.12.0`, `22`, `22.1`), resolved to
    /// the newest installed `~/.nvm/versions/node/v<that>...`.
    fn nvm_default(home: &Path) -> Option<PathBuf> {
        let nvm = home.join(".nvm");
        let alias = std::fs::read_to_string(nvm.join("alias").join("default")).ok()?;
        let want = alias.trim().trim_start_matches('v').to_owned();
        if want.is_empty() || !want.chars().next().is_some_and(|c| c.is_ascii_digit()) {
            return None;
        }
        let mut found: Vec<(Vec<u64>, PathBuf)> =
            std::fs::read_dir(nvm.join("versions").join("node"))
                .ok()?
                .flatten()
                .filter_map(|e| {
                    let name = e.file_name().to_string_lossy().into_owned();
                    let v = name.trim_start_matches('v');
                    let matches = v == want || v.starts_with(&format!("{want}."));
                    let node = e.path().join("bin").join(node_name());
                    (matches && node.is_file()).then(|| (version_key(v), node))
                })
                .collect();
        found.sort();
        found.pop().map(|(_, p)| p)
    }

    /// Find Node.js for `user` (the executable only: its version is read by [`node_version_output`] and checked by
    /// [`check_version`] on the caller's thread), or say where it was looked for.
    pub fn find(&self, user: &NodeUser) -> Result<(PathBuf, NodeSource), String> {
        let name = node_name();
        if let Some(c) = &self.configured {
            let candidate = if c.is_dir() { c.join(name) } else { c.clone() };
            return if candidate.is_file() {
                Ok((candidate, NodeSource::Configured))
            } else {
                Err(format!(
                    "{} ({NODE_ENV}) is {}, which is not Node.js; {} needs Node.js {} or later",
                    user.setting,
                    candidate.display(),
                    user.needs,
                    user.min_major
                ))
            };
        }
        if let Some(path) = &self.path
            && let Some(p) = std::env::split_paths(path)
                .map(|d| d.join(name))
                .find(|p| p.is_file())
        {
            return Ok((p, NodeSource::Path));
        }
        if let Some(home) = &self.home {
            let volta = home.join(".volta").join("bin").join(name);
            if volta.is_file() {
                return Ok((volta, NodeSource::Volta));
            }
            if let Some(p) = Self::nvm_default(home) {
                return Ok((p, NodeSource::Nvm));
            }
            for dir in [
                home.join(".local/share/fnm/aliases/default"),
                home.join(".fnm/aliases/default"),
                home.join("Library/Application Support/fnm/aliases/default"),
            ] {
                let p = dir.join("bin").join(name);
                if p.is_file() {
                    return Ok((p, NodeSource::Fnm));
                }
            }
        }
        Err(format!(
            "Node.js was not found: searched {} ({NODE_ENV}, not set), node on PATH, Volta (~/.volta/bin/node), nvm's \
             default (~/.nvm/alias/default) and fnm's default. {} runs on Node.js {} or later: install it \
             (https://nodejs.org, or your distribution's nodejs package) or set {}",
            user.setting, user.needs, user.min_major, user.setting
        ))
    }
}

/// `node --version`'s output (stdout and stderr), with stdin closed and a 10-second cap: run it off the UI thread.
pub fn node_version_output(node: &Path) -> Option<String> {
    command_output(node, &["--version"])
}

/// A command's stdout and stderr, stdin closed, killed after 10 seconds.
pub(crate) fn command_output(program: &Path, args: &[&str]) -> Option<String> {
    let mut child = Command::new(program)
        .no_console_window()
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .ok()?;
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(5)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
    let out = child.wait_with_output().ok()?;
    let mut text = String::from_utf8_lossy(&out.stdout).into_owned();
    text.push_str(&String::from_utf8_lossy(&out.stderr));
    Some(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn touch(p: &Path) {
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, b"").unwrap();
    }

    /// The order brief 0038 shipped for the debugger, now shared by the web servers (brief 0050): the setting, PATH,
    /// Volta, nvm, fnm; the messages name the user's setting.
    #[test]
    fn node_search_order_is_setting_path_volta_nvm_fnm() {
        let t = tempfile::tempdir().unwrap();
        let home = t.path().join("home");
        let on_path = t.path().join("onpath");
        let name = node_name();
        let mut s = NodeSearch {
            configured: None,
            path: Some(std::env::join_paths([t.path().join("nothing"), on_path.clone()]).unwrap()),
            home: Some(home.clone()),
        };
        let e = s.find(&WEB_SERVERS).unwrap_err();
        assert!(
            e.contains("Node.js was not found") && e.contains("languageServers.nodePath"),
            "{e}"
        );
        let fnm = home.join(".local/share/fnm/aliases/default/bin").join(name);
        touch(&fnm);
        assert_eq!(s.find(&WEB_SERVERS).unwrap(), (fnm, NodeSource::Fnm));
        std::fs::create_dir_all(home.join(".nvm/alias")).unwrap();
        std::fs::write(home.join(".nvm/alias/default"), "20\n").unwrap();
        touch(&home.join(".nvm/versions/node/v20.1.0/bin").join(name));
        touch(&home.join(".nvm/versions/node/v20.11.1/bin").join(name));
        touch(&home.join(".nvm/versions/node/v22.0.0/bin").join(name));
        assert_eq!(
            s.find(&WEB_SERVERS).unwrap(),
            (
                home.join(".nvm/versions/node/v20.11.1/bin").join(name),
                NodeSource::Nvm
            )
        );
        let volta = home.join(".volta/bin").join(name);
        touch(&volta);
        assert_eq!(s.find(&WEB_SERVERS).unwrap(), (volta, NodeSource::Volta));
        touch(&on_path.join(name));
        assert_eq!(
            s.find(&WEB_SERVERS).unwrap(),
            (on_path.join(name), NodeSource::Path)
        );
        let mine = t.path().join("mine");
        touch(&mine.join(name));
        s.configured = Some(mine.clone());
        assert_eq!(
            s.find(&WEB_SERVERS).unwrap(),
            (mine.join(name), NodeSource::Configured)
        );
        s.configured = Some(t.path().join("nope"));
        assert!(
            s.find(&WEB_SERVERS)
                .unwrap_err()
                .contains("languageServers.nodePath")
        );
    }

    #[test]
    fn versions_are_checked_against_the_users_minimum() {
        let node = Path::new("/n/node");
        assert_eq!(
            parse_node_version("v22.12.0\n"),
            Some(("v22.12.0".into(), 22))
        );
        assert_eq!(parse_node_version("node: bad option"), None);
        assert_eq!(
            check_version(node, Some("v20.0.0"), &WEB_SERVERS),
            Ok("v20.0.0".into())
        );
        let old = check_version(node, Some("v18.20.4"), &WEB_SERVERS).unwrap_err();
        assert!(
            old.contains("v18.20.4") && old.contains("20 or later"),
            "{old}"
        );
        // The minimum is tools/web-servers/PIN's.
        let pin = include_str!("../../../tools/web-servers/PIN");
        let node_line = pin.lines().find_map(|l| l.strip_prefix("node ")).unwrap();
        assert_eq!(node_line.trim(), WEB_SERVERS.min_major.to_string());
    }
}
