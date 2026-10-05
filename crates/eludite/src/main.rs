//! Eludite application entry point (PLAN.md D1, section 8, section 12 `crates/eludite`).
//!
//! Parses arguments and hands over to [`app::run`]. The window, docking and
//! command wiring live in `app` and `shell`; the docking model in
//! `eludite-docking`, the widgets in `eludite-ui`.

mod app;
mod args;
mod bench;
mod settings;
mod shell;
#[cfg(test)]
mod tests;

/// `--mcp-relay`: the stdio MCP server a hosted agent launches, relaying to the IDE's endpoint (brief 0016). stdout
/// carries MCP only; errors go to stderr.
fn relay(addr: std::net::SocketAddr) -> i32 {
    let Some(token) = std::env::var_os(eludite_mcp::transport::TOKEN_ENV) else {
        eprintln!(
            "eludite --mcp-relay: {} is not set",
            eludite_mcp::transport::TOKEN_ENV
        );
        return 2;
    };
    match eludite_mcp::transport::relay_stdio(addr, &token.to_string_lossy()) {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("eludite --mcp-relay {addr}: {e}");
            1
        }
    }
}

/// `--print-engine-discovery` (brief 0039, hidden): where the embedded engine and its CEF are found from here, how,
/// and how long the search took, as one JSON object on stdout; exit code 1 when either is missing. No window, no
/// settings file: the package smoke test runs it beside the packaged engine.
fn print_engine_discovery() -> i32 {
    let started = std::time::Instant::now();
    let search = eludite_browser::ChromiumSearch::defaults();
    let found = search.find();
    let elapsed_us = started.elapsed().as_micros() as u64;
    let (out, code) = match found {
        Ok(d) => (
            serde_json::json!({
                "engine": d.engine,
                "engine_found_by": d.engine_found.as_str(),
                "cef": d.cef,
                "cef_found_by": d.cef_found.as_str(),
                "elapsed_us": elapsed_us,
            }),
            0,
        ),
        Err(e) => (serde_json::json!({"error": e, "elapsed_us": elapsed_us}), 1),
    };
    println!("{out}");
    code
}

fn main() {
    let t_main = std::time::Instant::now();
    // Process-wide memory policy belongs here, not in a library (brief 0011).
    eludite_editor::syntax::alloc::disable_transparent_huge_pages();
    match args::Args::parse(std::env::args().skip(1)) {
        Ok(args) if args.help => print!("{}", args::USAGE),
        Ok(args) if args.print_engine_discovery => std::process::exit(print_engine_discovery()),
        // The update applier (brief 0055): a copy of the new executable, started by the shell before it quit.
        Ok(args) if args.apply_update.is_some() => {
            std::process::exit(eludite_update::apply::run(
                &args.apply_update.expect("checked"),
            ));
        }
        Ok(args) if args.mcp_relay.is_some() => {
            std::process::exit(relay(args.mcp_relay.expect("checked")));
        }
        Ok(args) => app::run(args, t_main),
        Err(e) => {
            eprintln!("eludite: {e}\n\n{}", args::USAGE);
            std::process::exit(2);
        }
    }
}
