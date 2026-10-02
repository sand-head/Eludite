//! `eludite-dbg-netfx`: a DAP server for .NET Framework over ICorDebug (PLAN.md 4.5, D7; brief 0004 spike).
//!
//! The adapter listens on TCP and serves one DAP client, which need not be on the same machine (ADR-0007): no pipes
//! or shared paths, and source paths are carried as given. On Windows the debugger is [`windows::CorDebugger`],
//! which attaches to a running .NET Framework 4 process through mscoree (`CLRCreateInstance` -> `ICLRMetaHost` ->
//! the process's loaded `ICLRRuntimeInfo` -> `ICorDebug`) and mscordbi. Elsewhere the crate builds with
//! [`Unsupported`], which answers `initialize` and refuses `attach`, so the protocol layer is tested on every OS.
//!
//! Public API boundary: [`cli`] (command line), [`serve_listener`] (accept one client and serve it), and the
//! protocol pieces [`framing`], [`protocol`], [`session`] and [`pdb`] (a portable PDB reader) for tests.

pub mod cli;
pub mod framing;
pub mod pdb;
pub mod protocol;
pub mod session;

#[cfg(windows)]
pub mod windows;

use std::io;
use std::net::TcpListener;

use protocol::{BreakpointResult, ScopeInfo, StackFrameInfo, ThreadInfo, VariableInfo};
use session::{DebugEvent, Debugger};

const UNSUPPORTED: &str = "eludite-dbg-netfx can only debug on Windows (ICorDebug); connect to an adapter running on a Windows machine";

/// The debugger on platforms without ICorDebug: every debugging request fails with an explanation.
#[derive(Debug, Default)]
pub struct Unsupported;

impl Debugger for Unsupported {
    type Event = ();
    fn attach(&mut self, _: u32) -> Result<(), String> {
        Err(UNSUPPORTED.into())
    }
    fn set_breakpoints(&mut self, _: &str, _: &[u32]) -> Result<Vec<BreakpointResult>, String> {
        Err(UNSUPPORTED.into())
    }
    fn configuration_done(&mut self) -> Result<(), String> {
        Ok(())
    }
    fn threads(&mut self) -> Result<Vec<ThreadInfo>, String> {
        Ok(Vec::new())
    }
    fn stack_trace(&mut self, _: u32) -> Result<Vec<StackFrameInfo>, String> {
        Err(UNSUPPORTED.into())
    }
    fn scopes(&mut self, _: i64) -> Result<Vec<ScopeInfo>, String> {
        Err(UNSUPPORTED.into())
    }
    fn variables(&mut self, _: i64) -> Result<Vec<VariableInfo>, String> {
        Err(UNSUPPORTED.into())
    }
    fn continue_all(&mut self) -> Result<(), String> {
        Err(UNSUPPORTED.into())
    }
    fn disconnect(&mut self, _: bool) -> Result<(), String> {
        Ok(())
    }
    fn on_event(&mut self, _: ()) -> Vec<DebugEvent> {
        Vec::new()
    }
}

/// Accepts one client on `listener` and serves it until it disconnects.
pub fn serve_listener(listener: TcpListener) -> io::Result<()> {
    let (stream, peer) = listener.accept()?;
    session::log(&format!("client connected from {peer}"));
    #[cfg(windows)]
    let r = session::serve(stream, windows::CorDebugger::new);
    #[cfg(not(windows))]
    let r = session::serve(stream, |_tx| Unsupported);
    session::log("session ended");
    r
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unsupported_refuses_attach_with_a_reason() {
        let e = Unsupported.attach(1).unwrap_err();
        assert!(e.contains("Windows"), "{e}");
    }
}
