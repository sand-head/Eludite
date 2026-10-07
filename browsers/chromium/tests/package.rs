//! Brief 0039's smoke test of the Linux package: `tools/package/linux.sh` lays Eludite out in a temporary folder
//! (the profile this test was built with, `--no-build`: the engine is this crate's, built with CEF; `eludite` is the
//! workspace's), the tarball lists the layout and its licenses, the `.desktop` file follows the freedesktop keys, and
//! then, with no `CEF_PATH`, `ELUDITE_CEF`, `ELUDITE_CHROMIUM`, `LD_LIBRARY_PATH` or CEF cache in their environment:
//!
//! - `eludite --print-engine-discovery` (a hidden flag for this test) finds the engine beside itself and CEF in `cef/`
//!   beside the engine, and says how long that took;
//! - `eludite-chromium` from the layout loads `cef/libcef.so` through its `$ORIGIN/cef` run path, answers
//!   `initialize`, reports the layout's CEF and its sandbox in `engine/ready`, and opens `about:blank`.
//!
//! As root (a container), or with `ELUDITE_CHROME_NO_SANDBOX=1` (CI), the engine gets `--allow-no-sandbox`, as the shell
//! passes it; the test says so. Skips with a message without the `cef` feature, off Linux, without CEF in tools/cef/fetch.sh's
//! cache (the script would download it), or when `target/<profile>/eludite` predates the flag (build the workspace).
//! `ELUDITE_PACKAGE_TARBALL=PATH` tests a tarball made by hand or by CI (`tools/package/linux.sh`, release) instead:
//! it is unpacked into a temporary folder first.

#![cfg(target_os = "linux")]

use std::io::{BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

/// The variables that would let the engine or the shell find CEF elsewhere than the layout.
const CLEARED: [&str; 7] = [
    "CEF_PATH",
    "ELUDITE_CEF",
    "ELUDITE_CHROMIUM",
    "LD_LIBRARY_PATH",
    "CEF_CACHE",
    "CARGO_TARGET_DIR",
    "ELUDITE_CHROME_NO_SANDBOX",
];

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn cef_version() -> String {
    std::fs::read_to_string(repo().join("tools/cef/PIN"))
        .unwrap()
        .lines()
        .find_map(|l| l.strip_prefix("version "))
        .unwrap()
        .trim()
        .to_owned()
}

fn cef_cached() -> bool {
    let root = std::env::var_os("CEF_CACHE")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache/eludite/cef")));
    root.is_some_and(|r| r.join(cef_version()).join("archive.json").is_file())
}

fn running_as_root() -> bool {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|s| {
            s.lines()
                .find_map(|l| l.strip_prefix("Uid:"))
                .and_then(|l| l.split_whitespace().nth(1).map(|u| u == "0"))
        })
        .unwrap_or(false)
}

/// A command with none of [`CLEARED`] and a `HOME` with no CEF cache.
fn clean(cmd: &mut Command, home: &Path) {
    for v in CLEARED {
        cmd.env_remove(v);
    }
    cmd.env("HOME", home);
}

fn send(w: &mut impl Write, v: &Value) {
    let body = serde_json::to_vec(v).unwrap();
    write!(w, "Content-Length: {}\r\n\r\n", body.len()).unwrap();
    w.write_all(&body).unwrap();
    w.flush().unwrap();
}

/// The layout to test, the tarball, and the folders to keep alive.
fn layout() -> Option<(PathBuf, PathBuf, Vec<tempfile::TempDir>)> {
    if let Some(tarball) = std::env::var_os("ELUDITE_PACKAGE_TARBALL").map(PathBuf::from) {
        let dir = tempfile::tempdir().unwrap();
        let st = Command::new("tar")
            .arg("-xzf")
            .arg(&tarball)
            .arg("-C")
            .arg(dir.path())
            .status()
            .unwrap();
        assert!(st.success(), "unpacking {}", tarball.display());
        let top = std::fs::read_dir(dir.path())
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        eprintln!(
            "testing {} unpacked at {}",
            tarball.display(),
            top.display()
        );
        return Some((top, tarball, vec![dir]));
    }
    let engine = Path::new(env!("CARGO_BIN_EXE_eludite-chromium"));
    let bin = engine.parent().unwrap();
    let profile = bin.file_name().unwrap().to_string_lossy().into_owned();
    let shell = bin.join("eludite");
    if !shell.is_file() {
        eprintln!(
            "skipped: no {} (cargo build -p eludite, or the whole workspace's tests)",
            shell.display()
        );
        return None;
    }
    let probe = Command::new(&shell)
        .arg("--print-engine-discovery")
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .unwrap();
    if String::from_utf8_lossy(&probe.stderr).contains("unknown argument") {
        eprintln!(
            "skipped: {} predates --print-engine-discovery (build it again)",
            shell.display()
        );
        return None;
    }
    let out = tempfile::tempdir().unwrap();
    let t = Instant::now();
    let run = Command::new("bash")
        .arg(repo().join("tools/package/linux.sh"))
        .args(["--profile", &profile, "--no-build", "--out"])
        .arg(out.path())
        // The script's own cargo calls (cargo tree) and its target folder follow this build's.
        .env("CARGO_TARGET_DIR", bin.parent().unwrap())
        .stderr(Stdio::inherit())
        .output()
        .unwrap();
    assert!(run.status.success(), "tools/package/linux.sh failed");
    let tarball = PathBuf::from(
        String::from_utf8_lossy(&run.stdout)
            .lines()
            .last()
            .unwrap()
            .trim(),
    );
    eprintln!(
        "tools/package/linux.sh --profile {profile} --no-build: {:?}",
        t.elapsed()
    );
    let name = tarball
        .file_name()
        .unwrap()
        .to_string_lossy()
        .trim_end_matches(".tar.gz")
        .to_owned();
    Some((out.path().join(name), tarball, vec![out]))
}

#[test]
fn the_package_runs_from_its_layout_with_no_variables() {
    if !cfg!(feature = "cef") {
        eprintln!("skipped: eludite-chromium was built without the cef feature");
        return;
    }
    if std::env::var_os("ELUDITE_PACKAGE_TARBALL").is_none() && !cef_cached() {
        eprintln!(
            "skipped: CEF {} is not in tools/cef/fetch.sh's cache (run it first)",
            cef_version()
        );
        return;
    }
    let Some((root, tarball, _keep)) = layout() else {
        return;
    };
    let root = root.canonicalize().unwrap();

    // The layout: the two executables, CEF's runtime files and licenses in cef/, the launcher, icons and README.
    for f in [
        "eludite",
        "eludite-chromium",
        "cef/libcef.so",
        "cef/chrome-sandbox",
        "cef/icudtl.dat",
        "cef/v8_context_snapshot.bin",
        "cef/resources.pak",
        "cef/chrome_100_percent.pak",
        "cef/chrome_200_percent.pak",
        "cef/locales/en-US.pak",
        "cef/libvk_swiftshader.so",
        "cef/vk_swiftshader_icd.json",
        "cef/libvulkan.so.1",
        "cef/LICENSE.txt",
        "cef/CREDITS.html",
        "eludite.desktop",
        "icons/hicolor/48x48/apps/eludite.png",
        "icons/hicolor/256x256/apps/eludite.png",
        "icons/hicolor/scalable/apps/eludite.svg",
        "README",
        "LICENSE",
        "THIRD-PARTY-CRATES.txt",
        "licenses/fonts/InstrumentSans-OFL.txt",
        "licenses/fonts/JetBrainsMono-OFL.txt",
    ] {
        assert!(root.join(f).exists(), "{f} is in the layout");
    }
    let license = std::fs::read_to_string(root.join("LICENSE")).unwrap();
    assert!(license.contains("GNU GENERAL PUBLIC LICENSE") && license.contains("Version 3"));
    let crates = std::fs::read_to_string(root.join("THIRD-PARTY-CRATES.txt")).unwrap();
    assert!(crates.contains("BSD-3-Clause") && crates.contains("cef/CREDITS.html"));
    assert!(
        crates.lines().any(|l| l.starts_with("gpui ")),
        "the linked crates"
    );
    let readme = std::fs::read_to_string(root.join("README")).unwrap();
    assert!(
        readme.contains(
            "sudo chown root:root cef/chrome-sandbox && sudo chmod 4755 cef/chrome-sandbox"
        ) && readme.contains("user namespaces")
            && readme.contains("browser.allowNoSandbox"),
        "the README names the sandbox rule"
    );
    // The freedesktop launcher (Desktop Entry Specification 1.5).
    let desktop = std::fs::read_to_string(root.join("eludite.desktop")).unwrap();
    assert!(desktop.starts_with("[Desktop Entry]\n"), "{desktop}");
    for key in [
        "Type=Application",
        "Name=Eludite",
        "Exec=eludite %F",
        "Icon=eludite",
        "Categories=Development;IDE;",
        "StartupWMClass=eludite",
    ] {
        assert!(
            desktop.lines().any(|l| l == key),
            "{key} in eludite.desktop"
        );
    }
    // The tarball holds the layout under its one folder.
    let listing = Command::new("tar")
        .arg("-tzf")
        .arg(&tarball)
        .output()
        .unwrap();
    assert!(listing.status.success());
    let listing = String::from_utf8_lossy(&listing.stdout);
    let top = root.file_name().unwrap().to_string_lossy();
    for f in [
        "eludite",
        "eludite-chromium",
        "cef/libcef.so",
        "cef/locales/en-US.pak",
        "README",
    ] {
        assert!(
            listing.lines().any(|l| l == format!("{top}/{f}")),
            "{top}/{f} is in the tarball"
        );
    }
    assert!(listing.lines().all(|l| l.starts_with(&format!("{top}/"))));
    let size = std::fs::metadata(&tarball).unwrap().len();
    eprintln!(
        "{}: {:.1} MB, {} entries",
        tarball.display(),
        size as f64 / 1048576.0,
        listing.lines().count()
    );

    let home = tempfile::tempdir().unwrap();

    // The shell finds the engine beside itself and CEF in cef/ beside the engine.
    let mut cmd = Command::new(root.join("eludite"));
    cmd.arg("--print-engine-discovery");
    clean(&mut cmd, home.path());
    let out = cmd.output().unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let found: Value = serde_json::from_slice(&out.stdout).unwrap();
    eprintln!("eludite --print-engine-discovery: {found}");
    assert_eq!(
        found["engine"],
        json!(root.join("eludite-chromium")),
        "{found}"
    );
    assert_eq!(found["engine_found_by"], "beside", "{found}");
    assert_eq!(found["cef"], json!(root.join("cef")), "{found}");
    assert_eq!(found["cef_found_by"], "beside", "{found}");

    // The engine from the layout: CEF through its run path, about:blank, the layout's CEF in engine/ready.
    let profile = tempfile::tempdir().unwrap();
    let mut cmd = Command::new(root.join("eludite-chromium"));
    cmd.arg("--profile").arg(profile.path());
    // As the shell passes it: for root, or with ELUDITE_CHROME_NO_SANDBOX=1 in this environment (CI's runners
    // restrict user namespaces); the engine still sandboxes whenever it can.
    let allow = running_as_root()
        || std::env::var("ELUDITE_CHROME_NO_SANDBOX").is_ok_and(|v| v.trim() == "1");
    if allow {
        eprintln!(
            "running as root or with ELUDITE_CHROME_NO_SANDBOX=1: passing --allow-no-sandbox to the packaged engine"
        );
        cmd.arg("--allow-no-sandbox");
    }
    clean(&mut cmd, home.path());
    let started = Instant::now();
    let mut child = cmd
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let stdout = child.stdout.take().unwrap();
    let stderr = child.stderr.take().unwrap();
    let errs = std::thread::spawn(move || {
        let mut s = String::new();
        let _ = std::io::Read::read_to_string(&mut BufReader::new(stderr), &mut s);
        s
    });
    let (tx, rx) = std::sync::mpsc::channel::<Value>();
    std::thread::spawn(move || {
        let mut r = BufReader::new(stdout);
        while let Ok(Some(body)) = eludite_protocol::framing::read_message(&mut r) {
            if tx.send(serde_json::from_slice(&body).unwrap()).is_err() {
                break;
            }
        }
    });
    let wait = |what: &str, pred: &dyn Fn(&Value) -> bool| -> Value {
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            match rx.recv_timeout(left) {
                Ok(m) if pred(&m) => return m,
                Ok(_) => {}
                Err(_) => panic!("no {what} from the packaged engine"),
            }
        }
    };
    send(
        &mut stdin,
        &json!({"jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": {"clientName": "package-test", "clientVersion": "0", "protocolVersion": 1}}),
    );
    let init = wait("initialize", &|m| m["id"] == 1);
    assert!(
        init["result"]["cefVersion"]
            .as_str()
            .unwrap()
            .starts_with("154.0.32"),
        "{init}"
    );
    eprintln!(
        "the packaged engine answered initialize in {:?}",
        started.elapsed()
    );
    let ready = wait("engine/ready", &|m| m["method"] == "engine/ready");
    let cef_dir = PathBuf::from(ready["params"]["cefDir"].as_str().unwrap());
    assert_eq!(
        cef_dir.canonicalize().unwrap(),
        root.join("cef").canonicalize().unwrap(),
        "{ready}"
    );
    let mode = ready["params"]["sandbox"].as_str().unwrap().to_owned();
    if running_as_root() {
        assert_eq!(mode, "none", "{ready}");
    }
    eprintln!("the packaged engine's sandbox: {mode}");
    send(
        &mut stdin,
        &json!({"jsonrpc": "2.0", "id": 2, "method": "tab/create",
            "params": {"url": "about:blank", "width": 320, "height": 200}}),
    );
    let tab = wait("tab/create", &|m| m["id"] == 2);
    let tab = tab["result"]["tab"].as_str().unwrap().to_owned();
    send(
        &mut stdin,
        &json!({"jsonrpc": "2.0", "method": "tab/cdp", "params": {"tab": tab,
            "message": {"id": 1, "method": "Runtime.evaluate", "params": {"expression": "location.href"}}}}),
    );
    let href = wait("Runtime.evaluate", &|m| {
        m["method"] == "tab/cdpEvent" && m["params"]["message"]["id"] == 1
    });
    assert_eq!(
        href["params"]["message"]["result"]["result"]["value"], "about:blank",
        "{href}"
    );
    send(
        &mut stdin,
        &json!({"jsonrpc": "2.0", "id": 3, "method": "shutdown", "params": {}}),
    );
    wait("shutdown", &|m| m["id"] == 3);
    drop(stdin);
    let status = child.wait().unwrap();
    let errs = errs.join().unwrap();
    assert!(status.success(), "{status}: {errs}");
    // The engine loaded the layout's libcef.so, not another one.
    assert!(
        !errs.contains("error while loading shared libraries"),
        "{errs}"
    );
}
