//! Integrated terminal (PLAN.md D2, 4.11, section 12 `crates/terminal`).
//!
//! Will host a PTY and a terminal emulator drawn by our own GPUI view (not
//! Zed's `terminal_view`, per D1). No PTY yet: only the launch configuration.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalConfig {
    pub shell: String,
    pub args: Vec<String>,
    /// `None` means the workspace root (or the process cwd without a workspace).
    pub cwd: Option<PathBuf>,
}

impl Default for TerminalConfig {
    fn default() -> Self {
        Self::platform_default()
    }
}

impl TerminalConfig {
    /// PowerShell on Windows; `$SHELL` (falling back to `/bin/sh`) elsewhere.
    pub fn platform_default() -> Self {
        if cfg!(windows) {
            Self {
                shell: "powershell.exe".into(),
                args: vec!["-NoLogo".into()],
                cwd: None,
            }
        } else {
            Self {
                shell: std::env::var("SHELL")
                    .ok()
                    .filter(|s| !s.is_empty())
                    .unwrap_or_else(|| "/bin/sh".into()),
                args: vec!["-l".into()],
                cwd: None,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_has_a_shell() {
        let c = TerminalConfig::default();
        assert!(!c.shell.is_empty());
        assert!(c.cwd.is_none());
    }
}
