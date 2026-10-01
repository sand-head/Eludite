//! `niello-fake-claude`: test-only stand-in for `claude`; see
//! `niello_claude_acp::fake_claude`.

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    std::process::exit(niello_claude_acp::fake_claude::main(args));
}
