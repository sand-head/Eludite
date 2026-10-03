//! `eludite-chromium`: see the library's documentation. Without the `cef` feature (and off Linux, for now) this
//! executable only says how to build the engine.

#[cfg(all(feature = "cef", target_os = "linux"))]
fn main() -> std::process::ExitCode {
    eludite_chromium::engine::main()
}

#[cfg(not(all(feature = "cef", target_os = "linux")))]
fn main() -> std::process::ExitCode {
    eprintln!(
        "{} was built without CEF. Fetch it with tools/cef/fetch.sh, then build the engine with \
         CEF_PATH=\"$(tools/cef/fetch.sh)\" cargo build -p eludite-chromium --features eludite-chromium/cef \
         (Linux; Windows and macOS are not built yet).",
        eludite_chromium::NAME
    );
    std::process::ExitCode::from(3)
}
