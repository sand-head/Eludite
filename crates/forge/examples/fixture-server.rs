//! Serve fixture folders on loopback until killed (brief 0046's Xvfb run points `forge.hosts` at it): prints the
//! server's base url as stdout's first line, and each request it answered to stderr.
//!
//! Usage: `cargo run -p eludite-forge --example fixture-server -- DIR [DIR...]`

use std::io::Write as _;
use std::path::PathBuf;
use std::time::Duration;

use eludite_forge::replay::{FixtureServer, Fixtures};

fn main() {
    let dirs: Vec<PathBuf> = std::env::args_os().skip(1).map(PathBuf::from).collect();
    let Some(first) = dirs.first() else {
        eprintln!("usage: fixture-server DIR [DIR...]");
        std::process::exit(2);
    };
    let fixtures = Fixtures::load(first).expect("the fixtures load");
    for d in &dirs[1..] {
        fixtures.add_dir(d).expect("the fixtures load");
    }
    let server = FixtureServer::start(fixtures).expect("the server starts");
    println!("{}", server.base());
    let _ = std::io::stdout().flush();
    let mut told = 0;
    loop {
        std::thread::sleep(Duration::from_millis(200));
        let seen = server.fixtures.seen();
        for s in seen.iter().skip(told) {
            eprintln!("{} {} {}", s.status, s.method, s.target);
        }
        told = seen.len();
    }
}
