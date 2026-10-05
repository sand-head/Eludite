//! Shell integration (brief 0041): a small script per shell, compiled in, written to a folder of Eludite's own and
//! passed to the shell through its startup options, so it runs the user's own startup files and then marks prompts
//! and commands with OSC 133 and reports the folder with OSC 7. The user's dotfiles are never written:
//!
//! - bash: `--rcfile <dir>/bash.sh` (it sources `~/.bashrc`, or the login files for a login shell);
//! - zsh: `ZDOTDIR=<dir>/zsh`, whose files source the user's from `ELUDITE_USER_ZDOTDIR` (their `ZDOTDIR`, or home);
//! - fish: `--init-command 'source <dir>/eludite.fish'` (after the user's config);
//! - PowerShell: `-NoExit -Command ". '<dir>\eludite.ps1'"` (after the profile, and after a Developer PowerShell's
//!   own `-Command`).
//!
//! `sh`, `dash`, `cmd.exe` and anything else get nothing: the prompt heuristic applies to them.

use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};

use crate::profile::ShellKind;

/// The scripts, by their path under the integration folder.
pub const SCRIPTS: [(&str, &str); 7] = [
    ("bash.sh", include_str!("../shell-integration/bash.sh")),
    (
        "zsh/.zshenv",
        include_str!("../shell-integration/zsh/.zshenv"),
    ),
    (
        "zsh/.zprofile",
        include_str!("../shell-integration/zsh/.zprofile"),
    ),
    (
        "zsh/.zshrc",
        include_str!("../shell-integration/zsh/.zshrc"),
    ),
    (
        "zsh/.zlogin",
        include_str!("../shell-integration/zsh/.zlogin"),
    ),
    (
        "eludite.fish",
        include_str!("../shell-integration/eludite.fish"),
    ),
    (
        "eludite.ps1",
        include_str!("../shell-integration/eludite.ps1"),
    ),
];

/// Where the scripts go: `<temp>/eludite-shell-integration-<user>/<version>`.
pub fn default_dir() -> PathBuf {
    default_dir_in(&|k| std::env::var(k).ok())
}

/// [`default_dir`] from the environment `env`: Eludite's cache folder (`$XDG_CACHE_HOME/eludite`, else
/// `~/.cache/eludite`; `%LOCALAPPDATA%\eludite` on Windows), never a folder other users can write to; the temporary
/// folder only when neither is known.
pub fn default_dir_in(env: &dyn Fn(&str) -> Option<String>) -> PathBuf {
    let cache = if cfg!(windows) {
        env("LOCALAPPDATA").map(PathBuf::from)
    } else {
        env("XDG_CACHE_HOME")
            .filter(|d| !d.is_empty())
            .map(PathBuf::from)
            .or_else(|| env("HOME").map(|h| Path::new(&h).join(".cache")))
    };
    match cache {
        Some(c) => c.join("eludite").join("shell-integration"),
        None => {
            let user = env("USER")
                .or_else(|| env("USERNAME"))
                .unwrap_or_else(|| "user".into());
            std::env::temp_dir().join(format!("eludite-shell-integration-{user}"))
        }
    }
    .join(env!("CARGO_PKG_VERSION"))
}

/// Write the scripts under `dir`, rewriting any that differ. Called off the UI thread (when a terminal opens).
pub fn install(dir: &Path) -> io::Result<()> {
    for (rel, text) in SCRIPTS {
        let path = dir.join(rel);
        if std::fs::read_to_string(&path).is_ok_and(|t| t == text) {
            continue;
        }
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let tmp = path.with_extension("tmp");
        std::fs::write(&tmp, text)?;
        std::fs::rename(&tmp, &path)?;
    }
    Ok(())
}

/// Add the integration of `kind` to a launch (`args` and `env` of the shell), with the scripts in `dir`. Returns
/// whether the shell gets the integration. `env_of` reads Eludite's environment (`ZDOTDIR`, `HOME`).
pub fn apply(
    kind: ShellKind,
    dir: &Path,
    args: &mut Vec<String>,
    env: &mut BTreeMap<String, String>,
    env_of: &dyn Fn(&str) -> Option<String>,
) -> bool {
    let path = |rel: &str| dir.join(rel).to_string_lossy().into_owned();
    match kind {
        ShellKind::Bash => {
            // `--rcfile` is ignored by login shells: imitate `-l` in the script instead.
            let login = args.iter().any(|a| a == "-l" || a == "--login");
            if args
                .iter()
                .any(|a| a == "-c" || a == "--norc" || a == "--rcfile")
            {
                return false;
            }
            args.retain(|a| a != "-l" && a != "--login");
            if login {
                env.insert("ELUDITE_SHELL_LOGIN".into(), "1".into());
            }
            args.insert(0, path("bash.sh"));
            args.insert(0, "--rcfile".into());
            if !args.iter().any(|a| a == "-i") {
                args.push("-i".into());
            }
            true
        }
        ShellKind::Zsh => {
            if args.iter().any(|a| a == "-c") {
                return false;
            }
            let user = env
                .get("ZDOTDIR")
                .cloned()
                .or_else(|| env_of("ZDOTDIR"))
                .or_else(|| env_of("HOME"))
                .unwrap_or_default();
            env.insert("ELUDITE_USER_ZDOTDIR".into(), user);
            env.insert("ZDOTDIR".into(), path("zsh"));
            true
        }
        ShellKind::Fish => {
            if args.iter().any(|a| a == "-c" || a == "--command") {
                return false;
            }
            args.push("--init-command".into());
            args.push(format!("source '{}'", path("eludite.fish")));
            true
        }
        ShellKind::PowerShell => {
            if args.iter().any(|a| a.eq_ignore_ascii_case("-File")) {
                return false;
            }
            let source = format!(". '{}'", path("eludite.ps1").replace('\'', "''"));
            match args
                .iter()
                .position(|a| a.eq_ignore_ascii_case("-Command") || a.eq_ignore_ascii_case("-c"))
            {
                // A Developer PowerShell's own command runs first.
                Some(i) if i + 1 < args.len() => {
                    args[i + 1] = format!("{}; {source}", args[i + 1]);
                }
                Some(_) => return false,
                None => {
                    args.push("-Command".into());
                    args.push(source);
                }
            }
            if !args.iter().any(|a| a.eq_ignore_ascii_case("-NoExit")) {
                args.insert(0, "-NoExit".into());
            }
            true
        }
        ShellKind::Cmd | ShellKind::Other => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn launch(kind: ShellKind, args: &[&str]) -> (bool, Vec<String>, BTreeMap<String, String>) {
        let mut a: Vec<String> = args.iter().map(|s| (*s).to_owned()).collect();
        let mut env = BTreeMap::new();
        let env_of = |k: &str| (k == "HOME").then(|| "/home/me".to_owned());
        let on = apply(kind, Path::new("/i"), &mut a, &mut env, &env_of);
        (on, a, env)
    }

    /// `rel` under the test's `/i`, joined as the platform does (`\\` on Windows).
    fn in_dir(rel: &str) -> String {
        Path::new("/i").join(rel).to_string_lossy().into_owned()
    }

    #[test]
    fn each_shell_gets_its_startup_option_and_never_a_dotfile() {
        let (on, args, env) = launch(ShellKind::Bash, &["-l"]);
        assert!(on);
        assert_eq!(args, ["--rcfile", &in_dir("bash.sh"), "-i"]);
        assert_eq!(env["ELUDITE_SHELL_LOGIN"], "1");
        let (on, _, env) = launch(ShellKind::Zsh, &[]);
        assert!(on);
        assert_eq!(env["ZDOTDIR"], in_dir("zsh"));
        assert_eq!(env["ELUDITE_USER_ZDOTDIR"], "/home/me");
        let (on, args, _) = launch(ShellKind::Fish, &[]);
        assert!(on);
        assert_eq!(
            args,
            [
                "--init-command",
                &format!("source '{}'", in_dir("eludite.fish"))
            ]
        );
        let (on, args, _) = launch(ShellKind::PowerShell, &["-NoLogo"]);
        assert!(on);
        assert_eq!(
            args,
            [
                "-NoExit",
                "-NoLogo",
                "-Command",
                &format!(". '{}'", in_dir("eludite.ps1"))
            ]
        );
        // A Developer PowerShell's command runs first.
        let (_, args, _) = launch(
            ShellKind::PowerShell,
            &["-NoLogo", "-NoExit", "-Command", "& 'x.ps1'"],
        );
        assert_eq!(args[3], format!("& 'x.ps1'; . '{}'", in_dir("eludite.ps1")));
        assert!(!launch(ShellKind::Other, &[]).0);
        assert!(!launch(ShellKind::Cmd, &[]).0);
        // A shell told to run a command is left alone.
        assert!(!launch(ShellKind::Bash, &["-c", "true"]).0);
    }

    #[test]
    fn the_scripts_live_in_eludites_cache_folder() {
        let env = |k: &str| (k == "HOME").then(|| "/home/me".to_owned());
        let dir = default_dir_in(&env);
        if !cfg!(windows) {
            assert!(
                dir.starts_with("/home/me/.cache/eludite/shell-integration"),
                "{dir:?}"
            );
        }
        let xdg = |k: &str| (k == "XDG_CACHE_HOME").then(|| "/c".to_owned());
        if !cfg!(windows) {
            assert!(default_dir_in(&xdg).starts_with("/c/eludite"));
        }
    }

    #[test]
    fn install_writes_the_scripts_once() {
        let dir = tempfile::tempdir().unwrap();
        install(dir.path()).unwrap();
        for (rel, text) in SCRIPTS {
            assert_eq!(std::fs::read_to_string(dir.path().join(rel)).unwrap(), text);
        }
        install(dir.path()).unwrap();
    }
}
