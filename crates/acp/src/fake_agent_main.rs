//! Scripted ACP agent on stdio, for tests and benchmarks. See
//! `eludite_acp::fake_agent`. Not shipped to users.

fn main() {
    let opts = match eludite_acp::fake_agent::Options::from_args(std::env::args().skip(1)) {
        Ok(o) => o,
        Err(e) => {
            eprintln!("eludite-fake-acp-agent: {e}");
            std::process::exit(2);
        }
    };
    let stdin = std::io::stdin().lock();
    let stdout = std::io::stdout().lock();
    if let Err(e) = eludite_acp::fake_agent::run(stdin, stdout, opts) {
        eprintln!("eludite-fake-acp-agent: {e}");
        std::process::exit(1);
    }
}
