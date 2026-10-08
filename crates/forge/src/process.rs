//! Console programs started from the windowed shell, without a console window each (the same as `eludite-lsp`'s
//! `process`, which this crate does not depend on).
//!
//! On Windows the shell is a GUI program (no console of its own), so every console program it starts (the host, a
//! language server, a debug adapter, `cargo`, `node`) would otherwise open its own console window. Its output goes to
//! pipes either way. Elsewhere this does nothing.

use std::process::Command;

/// [`no_console_window`](NoConsoleWindow::no_console_window) on a [`Command`].
pub(crate) trait NoConsoleWindow {
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
