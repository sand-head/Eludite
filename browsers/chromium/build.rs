//! With the `cef` feature on Linux, the engine finds `libcef.so` beside itself (`$ORIGIN`: the `cef-dll-sys` build
//! script copies CEF's runtime files into the target folder) or in `cef/` beside itself (`$ORIGIN/cef`: the package
//! layout of `tools/package/linux.sh`, brief 0039).
fn main() {
    println!("cargo::rerun-if-changed=build.rs");
    let cef = std::env::var_os("CARGO_FEATURE_CEF").is_some();
    let linux = std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("linux");
    if cef && linux {
        println!("cargo::rustc-link-arg-bins=-Wl,-rpath,$ORIGIN:$ORIGIN/cef");
    }
}
