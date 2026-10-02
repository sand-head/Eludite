//! Niello application entry point (PLAN.md D1, section 8, section 12 `crates/niello`).
//!
//! Parses arguments and hands over to [`app::run`]. The window, docking and
//! command wiring live in `app` and `shell`; the docking model in
//! `niello-docking`, the widgets in `niello-ui`.

mod app;
mod args;
mod bench;
mod shell;
#[cfg(test)]
mod tests;

fn main() {
    let t_main = std::time::Instant::now();
    match args::Args::parse(std::env::args().skip(1)) {
        Ok(args) if args.help => print!("{}", args::USAGE),
        Ok(args) => app::run(args, t_main),
        Err(e) => {
            eprintln!("niello: {e}\n\n{}", args::USAGE);
            std::process::exit(2);
        }
    }
}
