//! Terminal profiles: what a New Terminal runs. The built-in ones are the platform's (Visual Studio's names on
//! Windows: Developer PowerShell, PowerShell, Command Prompt and, with Build Tools, Developer Command Prompt; the
//! user's `$SHELL` and bash, zsh, fish and sh elsewhere, those found on PATH); the setting `terminal.profiles` adds
//! more, one with a built-in's name replacing it.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// One profile.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Profile {
    pub name: String,
    /// The executable: a path, or a name looked up on PATH.
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    /// The starting folder, absolute or relative to the workspace's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
}

impl Profile {
    pub fn new(name: impl Into<String>, command: impl Into<String>, args: &[&str]) -> Self {
        Self {
            name: name.into(),
            command: command.into(),
            args: args.iter().map(|a| (*a).to_owned()).collect(),
            env: BTreeMap::new(),
            cwd: None,
        }
    }

    /// The kind of shell it runs, from the executable's name.
    pub fn kind(&self) -> ShellKind {
        ShellKind::of(&self.command)
    }
}

/// The shells the integration knows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShellKind {
    Bash,
    Zsh,
    Fish,
    PowerShell,
    Cmd,
    /// `sh`, `dash` and anything else: no integration.
    Other,
}

impl ShellKind {
    /// From an executable's path or name (`/usr/bin/bash`, `pwsh.exe`).
    pub fn of(command: &str) -> Self {
        // Either separator, whatever the platform (a Windows path in a setting read on Linux).
        let name = command
            .rsplit(['/', '\\'])
            .next()
            .unwrap_or_default()
            .to_ascii_lowercase();
        let name = name.strip_suffix(".exe").unwrap_or(&name);
        match name {
            "bash" => ShellKind::Bash,
            "zsh" => ShellKind::Zsh,
            "fish" => ShellKind::Fish,
            "pwsh" | "powershell" => ShellKind::PowerShell,
            "cmd" => ShellKind::Cmd,
            _ => ShellKind::Other,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            ShellKind::Bash => "bash",
            ShellKind::Zsh => "zsh",
            ShellKind::Fish => "fish",
            ShellKind::PowerShell => "PowerShell",
            ShellKind::Cmd => "Command Prompt",
            ShellKind::Other => "sh",
        }
    }
}

/// The Visual Studio installation whose developer environment the Developer profiles enter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildTools {
    /// The installation folder (`C:\Program Files (x86)\Microsoft Visual Studio\2022\BuildTools`).
    pub install: PathBuf,
}

impl BuildTools {
    /// `Common7\Tools\Launch-VsDevShell.ps1`.
    pub fn dev_shell_script(&self) -> PathBuf {
        self.install
            .join("Common7")
            .join("Tools")
            .join("Launch-VsDevShell.ps1")
    }

    /// `Common7\Tools\VsDevCmd.bat`.
    pub fn dev_cmd(&self) -> PathBuf {
        self.install
            .join("Common7")
            .join("Tools")
            .join("VsDevCmd.bat")
    }

    /// Look in `ELUDITE_VS_INSTALL`, then the usual installation folders (newest version first; Build Tools, then
    /// the editions), for an installation with `Launch-VsDevShell.ps1`. No process is started (no `vswhere`).
    pub fn locate(env: &dyn Fn(&str) -> Option<String>) -> Option<Self> {
        let mut candidates = Vec::new();
        if let Some(dir) = env("ELUDITE_VS_INSTALL").filter(|d| !d.is_empty()) {
            candidates.push(PathBuf::from(dir));
        }
        for root in ["ProgramFiles(x86)", "ProgramFiles"]
            .iter()
            .filter_map(|v| env(v))
        {
            for year in ["18", "2022", "2019"] {
                for edition in ["BuildTools", "Enterprise", "Professional", "Community"] {
                    candidates.push(
                        Path::new(&root)
                            .join("Microsoft Visual Studio")
                            .join(year)
                            .join(edition),
                    );
                }
            }
        }
        candidates
            .into_iter()
            .map(|install| Self { install })
            .find(|b| b.dev_shell_script().is_file())
    }
}

/// Look `name` up on `path` (a PATH value), as a shell would.
pub fn which(name: &str, path: Option<&str>) -> Option<PathBuf> {
    if name.contains('/') || name.contains('\\') {
        let p = PathBuf::from(name);
        return p.is_file().then_some(p);
    }
    let exts: &[&str] = if cfg!(windows) {
        &[".exe", ".cmd", ".bat", ""]
    } else {
        &[""]
    };
    std::env::split_paths(path?).find_map(|dir| {
        exts.iter()
            .map(|e| dir.join(format!("{name}{e}")))
            .find(|p| p.is_file())
    })
}

/// The built-in profiles of this platform, the default first. `env` reads the environment (`SHELL`, `PATH`,
/// `ProgramFiles`).
pub fn builtin_profiles(env: &dyn Fn(&str) -> Option<String>) -> Vec<Profile> {
    if cfg!(windows) {
        windows_profiles(env, BuildTools::locate(env).as_ref())
    } else {
        unix_profiles(env)
    }
}

/// The user's `$SHELL` first (by its name), then bash, zsh, fish and sh where found on PATH.
pub fn unix_profiles(env: &dyn Fn(&str) -> Option<String>) -> Vec<Profile> {
    fn add(out: &mut Vec<Profile>, name: String, command: String) {
        if !out.iter().any(|p| p.name == name) {
            out.push(Profile {
                name,
                command,
                args: Vec::new(),
                env: BTreeMap::new(),
                cwd: None,
            });
        }
    }
    let path = env("PATH");
    let mut out: Vec<Profile> = Vec::new();
    if let Some(shell) = env("SHELL").filter(|s| !s.is_empty() && Path::new(s).is_file()) {
        let name = Path::new(&shell)
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| shell.clone());
        add(&mut out, name, shell);
    }
    for name in ["bash", "zsh", "fish", "sh"] {
        if let Some(p) = which(name, path.as_deref()) {
            add(&mut out, name.to_owned(), p.to_string_lossy().into_owned());
        }
    }
    if out.is_empty() {
        out.push(Profile::new("sh", "/bin/sh", &[]));
    }
    out
}

/// Visual Studio's: Developer PowerShell (entering the developer environment of `tools` when located, else a plain
/// PowerShell under that name), PowerShell, Command Prompt and, with `tools`, Developer Command Prompt.
pub fn windows_profiles(
    env: &dyn Fn(&str) -> Option<String>,
    tools: Option<&BuildTools>,
) -> Vec<Profile> {
    let path = env("PATH");
    let powershell = if which("pwsh", path.as_deref()).is_some() {
        "pwsh.exe"
    } else {
        "powershell.exe"
    };
    let mut dev = Profile::new("Developer PowerShell", powershell, &["-NoLogo"]);
    if let Some(t) = tools {
        // Launch-VsDevShell.ps1 enters the developer environment; -SkipAutomaticLocation keeps the folder.
        dev.args = vec![
            "-NoLogo".into(),
            "-NoExit".into(),
            "-Command".into(),
            format!(
                "& '{}' -SkipAutomaticLocation",
                t.dev_shell_script().display()
            ),
        ];
    }
    let mut out = vec![
        dev,
        Profile::new("PowerShell", powershell, &["-NoLogo"]),
        Profile::new("Command Prompt", "cmd.exe", &[]),
    ];
    if let Some(t) = tools {
        out.push(Profile::new(
            "Developer Command Prompt",
            "cmd.exe",
            &["/k", &t.dev_cmd().to_string_lossy()],
        ));
    }
    out
}

/// The built-in profiles with the user's added (`terminal.profiles`): one with a built-in's name replaces it.
pub fn merge_profiles(builtin: Vec<Profile>, added: &[Profile]) -> Vec<Profile> {
    let mut out = builtin;
    for p in added {
        match out.iter_mut().find(|b| b.name == p.name) {
            Some(b) => *b = p.clone(),
            None => out.push(p.clone()),
        }
    }
    out
}

/// The profile named `name` (or `default`, or the first) of `profiles`.
pub fn pick<'a>(profiles: &'a [Profile], name: Option<&str>, default: &str) -> Option<&'a Profile> {
    match name {
        Some(n) => profiles.iter().find(|p| p.name == n),
        None => profiles
            .iter()
            .find(|p| !default.is_empty() && p.name == default)
            .or_else(|| profiles.first()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kinds_come_from_the_executables_name() {
        assert_eq!(ShellKind::of("/usr/bin/bash"), ShellKind::Bash);
        assert_eq!(ShellKind::of("zsh"), ShellKind::Zsh);
        assert_eq!(ShellKind::of("/opt/homebrew/bin/fish"), ShellKind::Fish);
        assert_eq!(ShellKind::of("pwsh.exe"), ShellKind::PowerShell);
        assert_eq!(
            ShellKind::of(r"C:\Windows\powershell.exe"),
            ShellKind::PowerShell
        );
        assert_eq!(ShellKind::of("cmd.exe"), ShellKind::Cmd);
        assert_eq!(ShellKind::of("/bin/sh"), ShellKind::Other);
        assert_eq!(ShellKind::of("dash"), ShellKind::Other);
    }

    #[test]
    fn unix_profiles_put_the_users_shell_first() {
        let env = |k: &str| match k {
            "SHELL" => Some("/bin/sh".to_owned()),
            "PATH" => Some("/usr/bin:/bin".to_owned()),
            _ => None,
        };
        let p = unix_profiles(&env);
        assert_eq!(p[0].name, "sh");
        assert_eq!(p[0].command, "/bin/sh");
        assert_eq!(
            p.iter().filter(|x| x.name == "sh").count(),
            1,
            "no duplicate"
        );
        // Without $SHELL or PATH, sh.
        let none = |_: &str| None;
        assert_eq!(unix_profiles(&none)[0].command, "/bin/sh");
    }

    #[test]
    fn windows_profiles_use_visual_studios_names_and_build_tools_when_located() {
        let env = |_: &str| None;
        let plain = windows_profiles(&env, None);
        let names: Vec<_> = plain.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(
            names,
            ["Developer PowerShell", "PowerShell", "Command Prompt"]
        );
        assert_eq!(plain[0].args, ["-NoLogo"], "a plain PowerShell named so");
        let tools = BuildTools {
            install: PathBuf::from(r"C:\VS\BuildTools"),
        };
        let dev = windows_profiles(&env, Some(&tools));
        assert!(
            dev[0]
                .args
                .iter()
                .any(|a| a.contains("Launch-VsDevShell.ps1"))
        );
        assert!(dev[0].args.iter().any(|a| a == "-NoExit"));
        assert_eq!(dev[3].name, "Developer Command Prompt");
        assert!(dev[3].args[1].ends_with("VsDevCmd.bat"));
    }

    #[test]
    fn added_profiles_replace_by_name_and_pick_finds_the_default() {
        let builtin = vec![
            Profile::new("bash", "/bin/bash", &[]),
            Profile::new("sh", "/bin/sh", &[]),
        ];
        let added = [
            Profile::new("sh", "/usr/bin/dash", &[]),
            Profile::new("nu", "nu", &[]),
        ];
        let all = merge_profiles(builtin, &added);
        let names: Vec<_> = all.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["bash", "sh", "nu"]);
        assert_eq!(all[1].command, "/usr/bin/dash");
        assert_eq!(pick(&all, None, "").unwrap().name, "bash");
        assert_eq!(pick(&all, None, "nu").unwrap().name, "nu");
        assert_eq!(pick(&all, None, "gone").unwrap().name, "bash");
        assert_eq!(pick(&all, Some("sh"), "nu").unwrap().name, "sh");
        assert!(pick(&all, Some("gone"), "").is_none());
    }
}
