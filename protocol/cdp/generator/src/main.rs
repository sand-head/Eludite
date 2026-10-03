//! `cargo run -p eludite-cdp-generator`: regenerate `protocol/rust/src/cdp/` from `protocol/cdp/*.json` and print
//! what was generated. See the library's docs.

use std::process::ExitCode;

fn main() -> ExitCode {
    let scratch =
        std::env::temp_dir().join(format!("eludite-cdp-generator-{}", std::process::id()));
    let result =
        eludite_cdp_generator::generate_formatted(&eludite_cdp_generator::cdp_dir(), &scratch)
            .and_then(|g| {
                eludite_cdp_generator::write(&g.files, &eludite_cdp_generator::output_dir())
                    .map(|()| g)
            });
    let _ = std::fs::remove_dir_all(&scratch);
    match result {
        Ok(g) => {
            let lines: usize = g.files.values().map(|t| t.lines().count()).sum();
            let bytes: usize = g.files.values().map(String::len).sum();
            let s = &g.stats;
            println!(
                "eludite-cdp-generator: {} domains ({}), {} structs, {} enums, {} aliases, {} commands, {} events; {} files, {lines} lines, {bytes} bytes in {}",
                s.domains.len(),
                s.domains.join(", "),
                s.structs,
                s.enums,
                s.aliases,
                s.commands,
                s.events,
                g.files.len(),
                eludite_cdp_generator::output_dir().display()
            );
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("eludite-cdp-generator: {e}");
            ExitCode::FAILURE
        }
    }
}
