//! On Windows, embeds Eludite's icon (`tools/package/icons/eludite.ico`) in `eludite.exe` as icon resource 1: the
//! icon Explorer, the Start menu and the MSI's shortcut show, and the one GPUI's windows load for the title bar and
//! the taskbar. Nothing on other targets.

fn main() {
    println!("cargo:rerun-if-changed=eludite.rc");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let manifest_dir =
        std::path::PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").expect("set by cargo"));
    let icons = manifest_dir.join("../../tools/package/icons");
    println!(
        "cargo:rerun-if-changed={}",
        icons.join("eludite.ico").display()
    );
    embed_resource::compile("eludite.rc", embed_resource::ParamsIncludeDirs([icons]))
        .manifest_optional()
        .unwrap();
}
