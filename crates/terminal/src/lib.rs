//! The integrated terminal (brief 0041; PLAN.md 4.11, section 12 `crates/terminal`): a shell on a real PTY, its
//! output parsed by `alacritty_terminal` (Apache-2.0, from crates.io; never vendored, and never Zed's
//! `terminal_view`, per ADR-0001 and D1) and drawn by our own GPUI view.
//!
//! Public API:
//! - [`Terminal`] ([`pty`]): spawn a shell ([`Options`]), write to it, resize, clear and close it, read its screen,
//!   scrollback and plain-text transcript by [mark](text), and [`Terminal::wait`] for a prompt, a pattern or the
//!   exit (agents' `eludite.terminal.wait`). Its I/O thread raises [`Event`]s, coalesced to one `Changed` until the
//!   view draws.
//! - [`profile`]: the platform's profiles (Visual Studio's names on Windows, the Developer PowerShell entering a
//!   located Build Tools' environment), merged with the user's.
//! - [`env`]: the located tools' folders put first on PATH once ([`ToolPaths`]).
//! - [`integration`]: the shell integration scripts (OSC 133 marks, OSC 7 folder) for bash, zsh, fish and
//!   PowerShell, passed through the shells' startup options; [`scan`] takes those sequences out of the output.
//! - [`links`]: urls and `path(line,col)`, `path:line:col` links in a line.
//! - [`keys`]: keystrokes to the bytes a terminal sends (xterm's sequences).
//! - [`view`]: [`TerminalView`], the GPUI view: cells in the theme's 16 colors, the cursor, mouse selection, Visual
//!   Studio's clipboard keys, wheel and Shift+PageUp scrolling, find in the scrollback, the bell's flash, links on
//!   Ctrl+click, and the exit line with Restart.

pub mod env;
pub mod integration;
pub mod keys;
pub mod links;
pub mod profile;
pub mod pty;
pub mod scan;
pub mod text;
pub mod view;

pub use env::ToolPaths;
pub use profile::{Profile, ShellKind};
pub use pty::{Event, EventSink, Matched, Options, ScreenText, Terminal, WaitFor, WaitResult};
/// The regular expressions `Terminal::wait` takes (`WaitFor::pattern`).
pub use regex;
pub use view::{TerminalView, TerminalViewEvent, ViewSettings};
