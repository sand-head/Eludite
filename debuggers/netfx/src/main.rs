//! `eludite-dbg-netfx`: a DAP server for .NET Framework (PLAN.md 4.5, D7).
//!
//! Will drive the ICorDebug COM interfaces (`mscoree`/`mscordbi`) via the
//! `windows` crate, so it only runs on Windows. It is written as a DAP server
//! that does not assume its client is local (stdio, TCP or SSH-forwarded), which
//! is how Eludite on Linux/macOS debugs .NET Framework on a remote Windows box.

use std::process::ExitCode;

const UNSUPPORTED: &str = "eludite-dbg-netfx only runs on Windows (ICorDebug)";

#[cfg(not(windows))]
fn main() -> ExitCode {
    eprintln!("{UNSUPPORTED}");
    ExitCode::from(2)
}

#[cfg(windows)]
fn main() -> ExitCode {
    eprintln!("{UNSUPPORTED}");
    eprintln!("not implemented");
    ExitCode::from(2)
}
