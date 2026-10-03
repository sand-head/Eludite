//! `eludite-dbg-netfx`: a DAP server for .NET Framework over ICorDebug (PLAN.md 4.5, D7).
//!
//! Listens on TCP (`--listen HOST:PORT`, default `127.0.0.1:0`), prints the bound address to stderr and serves one
//! client. stdout is not used. See the library crate for the design.

use std::net::TcpListener;
use std::process::ExitCode;

use eludite_dbg_netfx::cli::{self, Cli};
use eludite_dbg_netfx::serve_listener;

fn main() -> ExitCode {
    let listen = match cli::parse(std::env::args().skip(1)) {
        Ok(Cli::Listen(a)) => a,
        Ok(Cli::Help) => {
            eprintln!("{}", cli::USAGE);
            return ExitCode::SUCCESS;
        }
        Err(e) => {
            eprintln!("eludite-dbg-netfx: {e}\n{}", cli::USAGE);
            return ExitCode::from(2);
        }
    };
    let listener = match TcpListener::bind(&listen) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("eludite-dbg-netfx: cannot listen on {listen}: {e}");
            return ExitCode::from(1);
        }
    };
    match listener.local_addr() {
        Ok(a) => eprintln!("eludite-dbg-netfx: listening on {a}"),
        Err(e) => {
            eprintln!("eludite-dbg-netfx: {e}");
            return ExitCode::from(1);
        }
    }
    match serve_listener(listener) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("eludite-dbg-netfx: {e}");
            ExitCode::from(1)
        }
    }
}
