//! `PIN` names the tree-sitter CLI `generate.sh` runs, and `build.rs` runs the
//! same generator as the `tree-sitter-generate` crate; they must be one version,
//! or the parser the build writes is not the one the CLI's test run proved.

#[test]
fn the_build_generator_is_the_pinned_cli_version() {
    let dir = env!("CARGO_MANIFEST_DIR");
    let pin = std::fs::read_to_string(format!("{dir}/PIN")).expect("PIN");
    let pin = pin.trim();
    let manifest = std::fs::read_to_string(format!("{dir}/Cargo.toml")).expect("Cargo.toml");
    let dependency = manifest
        .lines()
        .find(|line| line.starts_with("tree-sitter-generate"))
        .expect("Cargo.toml's tree-sitter-generate build dependency");
    assert!(
        dependency.contains(&format!("version = \"={pin}\"")),
        "Cargo.toml pins tree-sitter-generate as {dependency:?}; PIN says {pin}"
    );
}
