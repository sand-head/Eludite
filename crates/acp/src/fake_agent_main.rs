//! Scripted ACP agent on stdio, for tests and benchmarks. See
//! `niello_acp::fake_agent`. Not shipped to users.

fn main() {
    let opts = match niello_acp::fake_agent::Options::from_args(std::env::args().skip(1)) {
        Ok(o) => o,
        Err(e) => {
            eprintln!("niello-fake-acp-agent: {e}");
            std::process::exit(2);
        }
    };
    let stdin = std::io::stdin().lock();
    let stdout = std::io::stdout().lock();
    if let Err(e) = niello_acp::fake_agent::run(stdin, stdout, opts) {
        eprintln!("niello-fake-acp-agent: {e}");
        std::process::exit(1);
    }
}
