//! Console programs started from the windowed shell, without a console window each.
//!
//! On Windows the shell is a GUI program (no console of its own), so every console program it starts (the host, a
//! language server, a debug adapter, `cargo`, `node`) would otherwise open its own console window. Its output goes to
//! pipes either way. Elsewhere this does nothing.

use std::process::Command;

/// [`no_console_window`](NoConsoleWindow::no_console_window) on a [`Command`].
pub trait NoConsoleWindow {
    /// Start the program without a console window (`CREATE_NO_WINDOW` on Windows; nothing elsewhere).
    fn no_console_window(&mut self) -> &mut Self;
}

impl NoConsoleWindow for Command {
    fn no_console_window(&mut self) -> &mut Self {
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt as _;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            self.creation_flags(CREATE_NO_WINDOW);
        }
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_command_without_a_console_window_still_runs_with_its_output_piped() {
        let mut cmd = if cfg!(windows) {
            let mut c = Command::new("cmd");
            c.args(["/c", "echo eludite"]);
            c
        } else {
            let mut c = Command::new("sh");
            c.args(["-c", "echo eludite"]);
            c
        };
        let out = cmd.no_console_window().output().unwrap();
        assert!(out.status.success());
        assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "eludite");
    }
}
