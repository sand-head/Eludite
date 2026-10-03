//! The launch configuration of a Cargo package (brief 0029, `protocol/schemas/dap-lldb.md`): which executable to run,
//! with which arguments, environment and working directory, and the `launch` arguments lldb-dap gets, with the Rust
//! toolchain's LLDB formatters in `initCommands`.
//!
//! - **The binary** ([`CargoPackageInfo::binary`]): the package's only `bin` target, or the one `target` names, else
//!   refused listing them; its executable is `<target dir>/<debug|release>/<name>` ([`binary_path`]), where cargo puts
//!   (uplifts) a binary for the host.
//! - **The test executable** ([`test_build_command`], [`test_executable`]): `cargo test --no-run
//!   --message-format=json -p <package>` builds every test target of the package; its `compiler-artifact` messages with
//!   `profile.test` name each executable ([`parse_artifacts`]). The one debugged is `target`'s, else the library's
//!   unit tests, else the only binary's; it runs with the filter and `--nocapture`, as `cargo test` passes them.
//! - **Arguments and environment** ([`RunMetadata`]): `[package.metadata.eludite.run]` in the package's `Cargo.toml`,
//!   `args` (a list) and `env` (a table). The working directory is the workspace root, as for `cargo run`.
//! - **Formatters** ([`rust_init_commands`]): `rustc --print sysroot` in the workspace root ([`sysroot`], honoring
//!   `rust-toolchain.toml`), then what `rust-lldb` runs from `<sysroot>/lib/rustlib/etc`, after two lines that give
//!   LLDB 18's Python API the two methods the formatters of Rust 1.98 call.
//! - **Standard library sources** ([`map_rustc_path`]): frames in `std` carry `/rustc/<commit>/library/...`; with the
//!   `rust-src` component that is `<sysroot>/lib/rustlib/src/rust/library/...`.
//!
//! Everything that runs a process or reads a file is for the launch thread, never the UI thread.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde::Deserialize;
use serde_json::{Value, json};

/// What the launch configuration needs to know of a member package (from the shell's Cargo model).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CargoPackageInfo {
    pub name: String,
    /// The package's `Cargo.toml`.
    pub manifest: PathBuf,
    /// Its `bin` targets' names.
    pub bins: Vec<String>,
    /// Whether it has a library target (whose unit tests `test` debugs by default).
    pub has_lib: bool,
}

impl CargoPackageInfo {
    /// The binary target to run: `wanted`, else the only one; an error lists them otherwise.
    pub fn binary(&self, wanted: Option<&str>) -> Result<String, String> {
        match (wanted, self.bins.as_slice()) {
            (Some(w), bins) if bins.iter().any(|b| b == w) => Ok(w.to_owned()),
            (Some(w), []) => Err(format!(
                "the package {} has no binary target `{w}` (it has none)",
                self.name
            )),
            (Some(w), bins) => Err(format!(
                "the package {} has no binary target `{w}`; its binaries: {}",
                self.name,
                bins.join(", ")
            )),
            (None, [only]) => Ok(only.clone()),
            (None, []) => Err(format!(
                "the package {} has no binary target to run (debug its tests with `test: true`)",
                self.name
            )),
            (None, bins) => Err(format!(
                "the package {} has several binary targets; name one as `target`: {}",
                self.name,
                bins.join(", ")
            )),
        }
    }
}

/// `[package.metadata.eludite.run]`: how F5 and Ctrl+F5 run the package's binary.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct RunMetadata {
    pub args: Vec<String>,
    pub env: BTreeMap<String, String>,
}

impl RunMetadata {
    /// The table in a `Cargo.toml`'s text; none is the default. A malformed table is an error naming the file.
    pub fn parse(manifest_text: &str, manifest: &Path) -> Result<Self, String> {
        let value: toml::Value = toml::from_str(manifest_text)
            .map_err(|e| format!("{}: {}", manifest.display(), e.message()))?;
        let Some(run) = value
            .get("package")
            .and_then(|p| p.get("metadata"))
            .and_then(|m| m.get("eludite"))
            .and_then(|e| e.get("run"))
        else {
            return Ok(Self::default());
        };
        run.clone().try_into().map_err(|e: toml::de::Error| {
            format!(
                "{}: [package.metadata.eludite.run]: {} (`args` is a list of strings, `env` a table of strings)",
                manifest.display(),
                e.message()
            )
        })
    }

    /// Read and parse the package's `Cargo.toml` (the launch thread).
    pub fn read(manifest: &Path) -> Result<Self, String> {
        let text = std::fs::read_to_string(manifest)
            .map_err(|e| format!("{}: {e}", manifest.display()))?;
        Self::parse(&text, manifest)
    }
}

/// The profile folder of a configuration: `release` for Release, else `debug`.
pub fn profile_dir(release: bool) -> &'static str {
    if release { "release" } else { "debug" }
}

/// Where cargo puts binary `name` of the host: `<target dir>/<debug|release>/<name>` (`.exe` on Windows).
pub fn binary_path(target_dir: &Path, release: bool, name: &str) -> PathBuf {
    target_dir
        .join(profile_dir(release))
        .join(format!("{name}{}", std::env::consts::EXE_SUFFIX))
}

/// One `compiler-artifact` message of `cargo build` or `cargo test --no-run` with `--message-format=json`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Artifact {
    pub manifest: PathBuf,
    pub target: String,
    /// `bin`, `lib`, `test`, `example`, ... (the first of the target's kinds).
    pub kind: String,
    /// Built for tests (`profile.test`): the test harness.
    pub test: bool,
    pub executable: Option<PathBuf>,
}

/// The artifact messages among cargo's JSON lines (other lines and other reasons are skipped).
pub fn parse_artifacts(stdout: &str) -> Vec<Artifact> {
    stdout
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter(|v| v["reason"] == "compiler-artifact")
        .map(|v| Artifact {
            manifest: PathBuf::from(v["manifest_path"].as_str().unwrap_or_default()),
            target: v["target"]["name"].as_str().unwrap_or_default().to_owned(),
            kind: v["target"]["kind"][0]
                .as_str()
                .unwrap_or_default()
                .to_owned(),
            test: v["profile"]["test"].as_bool().unwrap_or(false),
            executable: v["executable"].as_str().map(PathBuf::from),
        })
        .collect()
}

/// The executable of a binary target in artifact messages (the build that F5 ran, when its messages are at hand).
pub fn binary_executable(artifacts: &[Artifact], manifest: &Path, name: &str) -> Option<PathBuf> {
    artifacts
        .iter()
        .filter(|a| same_path(&a.manifest, manifest) && a.kind == "bin" && !a.test)
        .find(|a| a.target == name)
        .and_then(|a| a.executable.clone())
}

/// The test executable to debug among `cargo test --no-run`'s artifacts: `target`'s (any test target by name), else
/// the library's unit tests, else the only one. An error lists the package's test executables otherwise.
pub fn test_executable(
    artifacts: &[Artifact],
    package: &CargoPackageInfo,
    target: Option<&str>,
) -> Result<PathBuf, String> {
    let tests: Vec<&Artifact> = artifacts
        .iter()
        .filter(|a| same_path(&a.manifest, &package.manifest) && a.test && a.executable.is_some())
        .collect();
    let pick = match target {
        Some(t) => tests.iter().find(|a| a.target == t),
        None => tests
            .iter()
            .find(|a| a.kind == "lib")
            .or_else(|| (tests.len() == 1).then(|| &tests[0])),
    };
    match pick {
        Some(a) => Ok(a.executable.clone().expect("filtered on executables")),
        None => {
            let names: Vec<String> = tests
                .iter()
                .map(|a| format!("{} ({})", a.target, a.kind))
                .collect();
            Err(if names.is_empty() {
                format!(
                    "cargo built no test executable for the package {}",
                    package.name
                )
            } else {
                format!(
                    "the package {} has several test executables; name one as `target`: {}",
                    package.name,
                    names.join(", ")
                )
            })
        }
    }
}

fn same_path(a: &Path, b: &Path) -> bool {
    a == b
        || std::fs::canonicalize(a)
            .ok()
            .zip(std::fs::canonicalize(b).ok())
            .is_some_and(|(x, y)| x == y)
}

/// The command that builds the package's test executables and names them: `cargo test --no-run
/// --message-format=json-diagnostic-rendered-ansi --manifest-path <workspace manifest> -p <package> [--release]`.
pub fn test_build_command(workspace_manifest: &Path, package: &str, release: bool) -> Vec<String> {
    let mut args = vec![
        "test".to_owned(),
        "--no-run".to_owned(),
        "--message-format=json-diagnostic-rendered-ansi".to_owned(),
        "--manifest-path".to_owned(),
        workspace_manifest.to_string_lossy().into_owned(),
        "-p".to_owned(),
        package.to_owned(),
    ];
    if release {
        args.push("--release".to_owned());
    }
    args
}

/// The arguments a test executable runs with: the filter and options given, then `--nocapture` (what
/// `cargo test <args> -- --nocapture` passes), so the test's output reaches the Debug source as it runs.
pub fn test_arguments(args: &[String]) -> Vec<String> {
    let mut a = args.to_vec();
    if !a.iter().any(|x| x == "--nocapture") {
        a.push("--nocapture".to_owned());
    }
    a
}

/// The `rustc` of the toolchain `cargo` uses: beside a configured cargo when there is one there, else `rustc` (the
/// rustup proxy on `PATH`, which reads `rust-toolchain.toml` from the working directory).
pub fn rustc_for(cargo: &Path) -> PathBuf {
    let name = format!("rustc{}", std::env::consts::EXE_SUFFIX);
    cargo
        .parent()
        .filter(|d| !d.as_os_str().is_empty())
        .map(|d| d.join(&name))
        .filter(|p| p.is_file())
        .unwrap_or_else(|| PathBuf::from(name))
}

/// `rustc --print sysroot` run in `cwd` (the workspace root, so `rust-toolchain.toml` picks the toolchain). Runs a
/// process: the launch thread only.
pub fn sysroot(rustc: &Path, cwd: &Path) -> Result<PathBuf, String> {
    let out = Command::new(rustc)
        .args(["--print", "sysroot"])
        .current_dir(cwd)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("{} --print sysroot: {e}", rustc.display()))?;
    let text = String::from_utf8_lossy(&out.stdout).trim().to_owned();
    if !out.status.success() || text.is_empty() {
        return Err(format!(
            "{} --print sysroot failed: {}",
            rustc.display(),
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(PathBuf::from(text))
}

/// Steps do not enter the standard library (`std`, `core`, `alloc`, and their trait implementations, whose names
/// start with `<`): LLDB's default is `^std::`.
pub const STEP_AVOID: &str =
    "settings set target.process.thread.step-avoid-regexp ^<?(std|core|alloc)::";

/// What LLDB 18's Python API lacks and the formatters of Rust 1.98 call (`SBValue.GetValueAsAddress`,
/// `SBValue.GetSyntheticValue`, LLDB 19). Each line defines its method only where it is missing.
pub const LLDB18_COMPAT: [&str; 2] = [
    "script if not hasattr(lldb.SBValue, 'GetValueAsAddress'): lldb.SBValue.GetValueAsAddress = lldb.SBValue.GetValueAsUnsigned",
    "script if not hasattr(lldb.SBValue, 'GetSyntheticValue'): lldb.SBValue.GetSyntheticValue = lambda self: (lambda v: (v.SetPreferSyntheticValue(True), v)[1] if v.IsValid() else v)(self.GetNonSyntheticValue())",
];

/// `<sysroot>/lib/rustlib/etc`, where the toolchain keeps its debugger scripts.
pub fn formatters_dir(sysroot: &Path) -> PathBuf {
    sysroot.join("lib").join("rustlib").join("etc")
}

/// The commands that load the Rust formatters from `etc` (`<sysroot>/lib/rustlib/etc`), as `rust-lldb` loads them:
/// the LLDB 18 compatibility lines, `command script import "<etc>/lldb_lookup.py"`, and `command source
/// "<etc>/lldb_commands"` when the toolchain has that file. None when `lldb_lookup.py` is missing.
pub fn rust_init_commands(etc: &Path) -> Vec<String> {
    let lookup = etc.join("lldb_lookup.py");
    if !lookup.is_file() {
        return Vec::new();
    }
    let mut c: Vec<String> = LLDB18_COMPAT.iter().map(|s| (*s).to_owned()).collect();
    c.push(format!("command script import \"{}\"", lldb_path(&lookup)));
    let commands = etc.join("lldb_commands");
    if commands.is_file() {
        c.push(format!("command source \"{}\"", lldb_path(&commands)));
    }
    c
}

/// A path for a double-quoted LLDB command argument, where a backslash escapes: Windows paths get forward slashes,
/// which LLDB accepts there.
fn lldb_path(path: &Path) -> String {
    let s = path.display().to_string();
    if cfg!(windows) {
        s.replace('\\', "/")
    } else {
        s
    }
}

/// `<sysroot>/lib/rustlib/src/rust` when the `rust-src` component is installed (it holds `library/`).
pub fn rust_src(sysroot: &Path) -> Option<PathBuf> {
    let dir = sysroot.join("lib").join("rustlib").join("src").join("rust");
    dir.join("library").is_dir().then_some(dir)
}

/// A standard library frame's path (`/rustc/<commit>/library/std/src/rt.rs`) under `rust_src` when the `rust-src`
/// component is installed (`<rust_src>/library/std/src/rt.rs`). `None` for any other path.
pub fn map_rustc_path(path: &str, rust_src: &Path) -> Option<PathBuf> {
    let rest = path
        .strip_prefix("/rustc/")
        .or_else(|| path.strip_prefix("\\rustc\\"))?;
    let (commit, rest) = rest.split_once(['/', '\\'])?;
    if commit.is_empty() || !commit.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    Some(rust_src.join(rest))
}

/// Whether `path` is a standard library source path as rustc records it (`/rustc/<commit>/...`).
pub fn is_rustc_path(path: &str) -> bool {
    map_rustc_path(path, Path::new("")).is_some()
}

/// Everything needed to run a Cargo package's executable under lldb-dap or without the debugger.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CargoLaunch {
    /// The package's `Cargo.toml` (the session's project).
    pub manifest: PathBuf,
    pub package: String,
    /// The binary or test target run.
    pub target: String,
    pub test: bool,
    pub program: PathBuf,
    pub args: Vec<String>,
    /// Added to the inherited environment.
    pub env: BTreeMap<String, String>,
    /// The workspace root.
    pub cwd: PathBuf,
}

impl CargoLaunch {
    /// lldb-dap's `launch` arguments (`protocol/schemas/dap-lldb.md`). `init_commands` are [`STEP_AVOID`] then the
    /// formatters' commands, as the launch thread computed them.
    pub fn lldb_arguments(&self, init_commands: &[String]) -> Value {
        json!({
            "name": format!("{} (Eludite)", self.target),
            "type": "lldb",
            "request": "launch",
            "program": self.program.to_string_lossy(),
            "args": self.args,
            "cwd": self.cwd.to_string_lossy(),
            // lldb-dap 18 takes `env` as a list of `NAME=value` strings.
            "env": self.env.iter().map(|(k, v)| format!("{k}={v}")).collect::<Vec<_>>(),
            "stopOnEntry": false,
            "initCommands": init_commands,
            "sourceMap": [],
        })
    }
}

/// What a start asks of a Cargo package, with what the shell's Cargo model knows of its workspace.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CargoStart {
    pub package: CargoPackageInfo,
    /// The workspace root (the working directory) and its `Cargo.toml`.
    pub root: PathBuf,
    pub workspace_manifest: PathBuf,
    /// `cargo metadata`'s `target_directory`.
    pub target_dir: PathBuf,
    /// The Release configuration (`--release`, `target/release`).
    pub release: bool,
    /// The binary (or, with `test`, the test target) to run.
    pub target: Option<String>,
    pub test: bool,
    /// Instead of `[package.metadata.eludite.run]`'s `args`; with `test` the harness's arguments.
    pub args: Option<Vec<String>>,
}

impl CargoStart {
    /// The launch of the package's binary target: the executable cargo wrote for it (`artifacts`, when the build's
    /// messages are at hand, else `<target dir>/<profile>/<name>`), which must exist, with the run table's arguments
    /// and environment. Reads `Cargo.toml`: the launch thread.
    pub fn binary_launch(&self, artifacts: &[Artifact]) -> Result<CargoLaunch, String> {
        let name = self.package.binary(self.target.as_deref())?;
        let run = RunMetadata::read(&self.package.manifest)?;
        let program = binary_executable(artifacts, &self.package.manifest, &name)
            .unwrap_or_else(|| binary_path(&self.target_dir, self.release, &name));
        if !program.is_file() {
            return Err(format!(
                "{} is not built: {} does not exist. Build it first (cargo build -p {}{}).",
                self.package.name,
                program.display(),
                self.package.name,
                if self.release { " --release" } else { "" }
            ));
        }
        Ok(CargoLaunch {
            manifest: self.package.manifest.clone(),
            package: self.package.name.clone(),
            target: name,
            test: false,
            program,
            args: self.args.clone().unwrap_or(run.args),
            env: run.env,
            cwd: self.root.clone(),
        })
    }

    /// The launch of the package's test executable, from `cargo test --no-run`'s artifact messages
    /// ([`CargoStart::build_tests`]): the filter and `--nocapture` as arguments, the run table's environment.
    pub fn test_launch(&self, artifacts: &[Artifact]) -> Result<CargoLaunch, String> {
        let program = test_executable(artifacts, &self.package, self.target.as_deref())?;
        let run = RunMetadata::read(&self.package.manifest)?;
        let target = artifacts
            .iter()
            .find(|a| a.executable.as_deref() == Some(program.as_path()))
            .map(|a| a.target.clone())
            .unwrap_or_else(|| self.package.name.clone());
        Ok(CargoLaunch {
            manifest: self.package.manifest.clone(),
            package: self.package.name.clone(),
            target,
            test: true,
            program,
            args: test_arguments(self.args.as_deref().unwrap_or_default()),
            env: run.env,
            cwd: self.root.clone(),
        })
    }

    /// Build the package's test executables with `cargo` (`cargo test --no-run --message-format=json...`), handing
    /// each line of cargo's progress and each compiler message's rendered text to `line` as it comes, and return the
    /// artifacts. A failed build is an error with its last error line. Runs cargo: the launch thread.
    pub fn build_tests(
        &self,
        cargo: &Path,
        mut line: impl FnMut(&str),
    ) -> Result<Vec<Artifact>, String> {
        use std::io::BufRead as _;
        let args = test_build_command(&self.workspace_manifest, &self.package.name, self.release);
        line(&format!("> {} {}", cargo.display(), args.join(" ")));
        let mut child = Command::new(cargo)
            .args(&args)
            .current_dir(&self.root)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| format!("{}: {e}", cargo.display()))?;
        let stderr = child.stderr.take().expect("piped stderr");
        let (tx, rx) = std::sync::mpsc::channel::<String>();
        let reader = std::thread::spawn(move || {
            for l in std::io::BufReader::new(stderr)
                .lines()
                .map_while(Result::ok)
            {
                if tx.send(l).is_err() {
                    break;
                }
            }
        });
        let mut stdout = String::new();
        let mut last_error = None;
        let out = child.stdout.take().expect("piped stdout");
        for l in std::io::BufReader::new(out).lines().map_while(Result::ok) {
            while let Ok(e) = rx.try_recv() {
                line(&e);
            }
            if let Ok(v) = serde_json::from_str::<Value>(&l) {
                if v["reason"] == "compiler-message" {
                    if let Some(r) = v["message"]["rendered"].as_str() {
                        line(r.trim_end());
                    }
                    if v["message"]["level"] == "error" {
                        last_error = v["message"]["message"].as_str().map(str::to_owned);
                    }
                }
                stdout.push_str(&l);
                stdout.push('\n');
            } else {
                line(&l);
            }
        }
        let _ = reader.join();
        for e in rx.try_iter() {
            line(&e);
        }
        let status = child.wait().map_err(|e| e.to_string())?;
        if !status.success() {
            return Err(format!(
                "the test build of {} failed{}",
                self.package.name,
                last_error.map(|e| format!(": {e}")).unwrap_or_default()
            ));
        }
        Ok(parse_artifacts(&stdout))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(p: &Path, text: &str) {
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, text).unwrap();
    }

    #[test]
    fn run_metadata_parses_args_and_env() {
        let m = Path::new("/w/app/Cargo.toml");
        let text = r#"
[package]
name = "app"
version = "0.1.0"

[package.metadata.eludite.run]
args = ["--verbose", "two words"]
env = { RUST_LOG = "debug", MODE = "test" }

[dependencies]
"#;
        let r = RunMetadata::parse(text, m).unwrap();
        assert_eq!(r.args, ["--verbose", "two words"]);
        assert_eq!(r.env["RUST_LOG"], "debug");
        assert_eq!(r.env["MODE"], "test");
        // A table section works too; a missing table is the default; other metadata is ignored.
        let r = RunMetadata::parse(
            "[package]\nname = \"a\"\n[package.metadata.eludite.run.env]\nX = \"1\"\n[package.metadata.docs.rs]\nall-features = true\n",
            m,
        )
        .unwrap();
        assert_eq!((r.args.len(), r.env["X"].as_str()), (0, "1"));
        assert_eq!(
            RunMetadata::parse("[package]\nname = \"a\"\n", m).unwrap(),
            RunMetadata::default()
        );
        // Wrong types name the table and what it wants.
        let err =
            RunMetadata::parse("[package.metadata.eludite.run]\nargs = \"--x\"\n", m).unwrap_err();
        assert!(
            err.contains("[package.metadata.eludite.run]") && err.contains("list of strings"),
            "{err}"
        );
        let err =
            RunMetadata::parse("[package.metadata.eludite.run]\nenv = { N = 1 }\n", m).unwrap_err();
        assert!(err.contains("Cargo.toml"), "{err}");
        assert!(
            RunMetadata::parse("[package", m)
                .unwrap_err()
                .contains("Cargo.toml")
        );
    }

    #[test]
    fn the_binary_target_is_the_only_one_or_the_named_one() {
        let mut p = CargoPackageInfo {
            name: "app".into(),
            manifest: "/w/app/Cargo.toml".into(),
            bins: vec!["app".into()],
            has_lib: false,
        };
        assert_eq!(p.binary(None).unwrap(), "app");
        assert_eq!(p.binary(Some("app")).unwrap(), "app");
        assert!(
            p.binary(Some("tool"))
                .unwrap_err()
                .contains("its binaries: app")
        );
        p.bins.push("tool".into());
        let err = p.binary(None).unwrap_err();
        assert!(
            err.contains("several") && err.contains("app, tool"),
            "{err}"
        );
        assert_eq!(p.binary(Some("tool")).unwrap(), "tool");
        p.bins.clear();
        assert!(p.binary(None).unwrap_err().contains("no binary target"));
        let exe = binary_path(Path::new("/w/target"), false, "app");
        assert_eq!(
            exe,
            Path::new("/w/target/debug").join(format!("app{}", std::env::consts::EXE_SUFFIX))
        );
        assert!(binary_path(Path::new("/w/target"), true, "app").starts_with("/w/target/release"));
    }

    /// Lines as `cargo test --no-run --message-format=json` printed them for a package with a library, a binary and
    /// an integration test (trimmed to the fields read).
    const RECORDED: &str = r#"{"reason":"compiler-artifact","package_id":"path+file:///w/app#0.1.0","manifest_path":"/w/app/Cargo.toml","target":{"kind":["lib"],"crate_types":["lib"],"name":"app","src_path":"/w/app/src/lib.rs"},"profile":{"opt_level":"0","debuginfo":2,"test":false},"features":[],"filenames":["/w/target/debug/deps/libapp-1.rlib"],"executable":null,"fresh":true}
{"reason":"compiler-artifact","package_id":"path+file:///w/app#0.1.0","manifest_path":"/w/app/Cargo.toml","target":{"kind":["lib"],"crate_types":["lib"],"name":"app","src_path":"/w/app/src/lib.rs"},"profile":{"opt_level":"0","debuginfo":2,"test":true},"features":[],"filenames":["/w/target/debug/deps/app-0f3a"],"executable":"/w/target/debug/deps/app-0f3a","fresh":false}
{"reason":"compiler-artifact","package_id":"path+file:///w/app#0.1.0","manifest_path":"/w/app/Cargo.toml","target":{"kind":["bin"],"crate_types":["bin"],"name":"app","src_path":"/w/app/src/main.rs"},"profile":{"opt_level":"0","debuginfo":2,"test":true},"features":[],"filenames":["/w/target/debug/deps/app-77b1"],"executable":"/w/target/debug/deps/app-77b1","fresh":false}
{"reason":"compiler-artifact","package_id":"path+file:///w/app#0.1.0","manifest_path":"/w/app/Cargo.toml","target":{"kind":["test"],"crate_types":["bin"],"name":"api","src_path":"/w/app/tests/api.rs"},"profile":{"opt_level":"0","debuginfo":2,"test":true},"features":[],"filenames":["/w/target/debug/deps/api-9c2e"],"executable":"/w/target/debug/deps/api-9c2e","fresh":false}
{"reason":"compiler-artifact","package_id":"path+file:///w/app#0.1.0","manifest_path":"/w/app/Cargo.toml","target":{"kind":["bin"],"crate_types":["bin"],"name":"app","src_path":"/w/app/src/main.rs"},"profile":{"opt_level":"0","debuginfo":2,"test":false},"features":[],"filenames":["/w/target/debug/app"],"executable":"/w/target/debug/app","fresh":true}
{"reason":"build-finished","success":true}
   Compiling app v0.1.0 (/w/app)
"#;

    #[test]
    fn executables_come_from_the_artifact_messages() {
        let a = parse_artifacts(RECORDED);
        assert_eq!(a.len(), 5);
        assert_eq!(a[0].executable, None);
        assert!(a[1].test && a[1].kind == "lib");
        let app = CargoPackageInfo {
            name: "app".into(),
            manifest: "/w/app/Cargo.toml".into(),
            bins: vec!["app".into()],
            has_lib: true,
        };
        let m = Path::new("/w/app/Cargo.toml");
        assert_eq!(
            binary_executable(&a, m, "app"),
            Some(PathBuf::from("/w/target/debug/app"))
        );
        assert_eq!(
            binary_executable(&a, Path::new("/w/other/Cargo.toml"), "app"),
            None
        );
        // The library's unit tests by default; any test target by name.
        assert_eq!(
            test_executable(&a, &app, None).unwrap(),
            Path::new("/w/target/debug/deps/app-0f3a")
        );
        assert_eq!(
            test_executable(&a, &app, Some("api")).unwrap(),
            Path::new("/w/target/debug/deps/api-9c2e")
        );
        // Without a library and with several test executables, the target must be named.
        let no_lib: Vec<Artifact> = a.iter().filter(|x| x.kind != "lib").cloned().collect();
        let err = test_executable(&no_lib, &app, None).unwrap_err();
        assert!(
            err.contains("app (bin)") && err.contains("api (test)"),
            "{err}"
        );
        let only_bin: Vec<Artifact> = no_lib.iter().filter(|x| x.kind == "bin").cloned().collect();
        assert_eq!(
            test_executable(&only_bin, &app, None).unwrap(),
            Path::new("/w/target/debug/deps/app-77b1")
        );
        assert!(
            test_executable(&[], &app, None)
                .unwrap_err()
                .contains("no test executable")
        );
    }

    #[test]
    fn the_test_command_line_and_arguments() {
        assert_eq!(
            test_build_command(Path::new("/w/Cargo.toml"), "app", true).join(" "),
            "test --no-run --message-format=json-diagnostic-rendered-ansi --manifest-path /w/Cargo.toml -p app --release"
        );
        assert!(
            !test_build_command(Path::new("/w/Cargo.toml"), "app", false)
                .contains(&"--release".to_owned())
        );
        assert_eq!(
            test_arguments(&["my_test".into()]),
            ["my_test", "--nocapture"]
        );
        assert_eq!(test_arguments(&["--nocapture".into()]), ["--nocapture"]);
        assert_eq!(test_arguments(&[]), ["--nocapture"]);
    }

    #[test]
    fn launch_arguments_load_the_formatters_from_the_sysroot() {
        let t = tempfile::tempdir().unwrap();
        let sysroot = t.path().join("toolchain");
        let etc = formatters_dir(&sysroot);
        // No formatters in the toolchain: no commands.
        assert!(rust_init_commands(&etc).is_empty());
        write(&etc.join("lldb_lookup.py"), "");
        let c = rust_init_commands(&etc);
        assert_eq!(c.len(), 3, "{c:?}");
        assert!(c[0].starts_with("script if not hasattr(lldb.SBValue, 'GetValueAsAddress')"));
        assert!(c[1].contains("GetSyntheticValue"));
        assert_eq!(
            c[2],
            format!(
                "command script import \"{}\"",
                lldb_path(&etc.join("lldb_lookup.py"))
            )
        );
        // A backslash escapes inside LLDB's quotes, so none reaches it.
        assert!(!c[2].contains('\\'), "{}", c[2]);
        // An older toolchain's lldb_commands is sourced after the import, as rust-lldb did.
        write(&etc.join("lldb_commands"), "");
        let c = rust_init_commands(&etc);
        assert_eq!(
            c[3],
            format!(
                "command source \"{}\"",
                lldb_path(&etc.join("lldb_commands"))
            )
        );

        let mut init = vec![STEP_AVOID.to_owned()];
        init.extend(c.clone());
        let launch = CargoLaunch {
            manifest: "/w/app/Cargo.toml".into(),
            package: "app".into(),
            target: "app".into(),
            test: false,
            program: "/w/target/debug/app".into(),
            args: vec!["--verbose".into()],
            env: BTreeMap::from([("RUST_LOG".to_owned(), "debug".to_owned())]),
            cwd: "/w".into(),
        };
        let v = launch.lldb_arguments(&init);
        assert_eq!(v["program"], "/w/target/debug/app");
        assert_eq!(v["args"], json!(["--verbose"]));
        assert_eq!(v["cwd"], "/w");
        assert_eq!(v["env"], json!(["RUST_LOG=debug"]));
        assert_eq!(v["stopOnEntry"], false);
        assert_eq!(v["sourceMap"], json!([]));
        assert_eq!(v["initCommands"][0], STEP_AVOID);
        assert_eq!(v["initCommands"].as_array().unwrap().len(), 5);

        // rust-src: the standard library's /rustc/<commit>/ paths map under it.
        assert_eq!(rust_src(&sysroot), None);
        let src = sysroot.join("lib/rustlib/src/rust");
        std::fs::create_dir_all(src.join("library")).unwrap();
        assert_eq!(rust_src(&sysroot), Some(src.clone()));
        assert_eq!(
            map_rustc_path(
                "/rustc/2d8144b7880597b6e6d3dfd63a9a9efae3f533d3/library/std/src/rt.rs",
                &src
            ),
            Some(src.join("library/std/src/rt.rs"))
        );
        assert_eq!(map_rustc_path("/home/me/app/src/main.rs", &src), None);
        assert_eq!(map_rustc_path("/rustc/not-hex/x.rs", &src), None);
        assert!(is_rustc_path(
            "/rustc/48a229ce/library/core/src/panicking.rs"
        ));
        assert!(!is_rustc_path("/w/app/src/main.rs"));
        // rustc beside a configured cargo, else the one on PATH.
        assert_eq!(
            rustc_for(Path::new("cargo")),
            PathBuf::from(format!("rustc{}", std::env::consts::EXE_SUFFIX))
        );
        let bin = t.path().join("bin");
        write(
            &bin.join(format!("rustc{}", std::env::consts::EXE_SUFFIX)),
            "",
        );
        assert_eq!(
            rustc_for(&bin.join("cargo")),
            bin.join(format!("rustc{}", std::env::consts::EXE_SUFFIX))
        );
    }

    #[test]
    fn a_start_resolves_the_binary_or_the_test_executable() {
        let t = tempfile::tempdir().unwrap();
        let root = t.path().to_path_buf();
        let manifest = root.join("app/Cargo.toml");
        write(
            &manifest,
            "[package]\nname = \"app\"\n[package.metadata.eludite.run]\nargs = [\"--port\", \"80\"]\nenv = { MODE = \"dev\" }\n",
        );
        let mut start = CargoStart {
            package: CargoPackageInfo {
                name: "app".into(),
                manifest: manifest.clone(),
                bins: vec!["app".into()],
                has_lib: false,
            },
            root: root.clone(),
            workspace_manifest: root.join("Cargo.toml"),
            target_dir: root.join("target"),
            release: false,
            target: None,
            test: false,
            args: None,
        };
        // Not built: the message says what to build.
        let err = start.binary_launch(&[]).unwrap_err();
        assert!(
            err.contains("cargo build -p app") && err.contains("debug"),
            "{err}"
        );
        let exe = binary_path(&root.join("target"), false, "app");
        write(&exe, "");
        let l = start.binary_launch(&[]).unwrap();
        assert_eq!(
            (l.program.clone(), l.cwd.clone(), l.test),
            (exe.clone(), root.clone(), false)
        );
        assert_eq!(l.args, ["--port", "80"]);
        assert_eq!(l.env["MODE"], "dev");
        // The artifact message's executable wins over the computed path; the start's args over the table's.
        let other = root.join("elsewhere/app");
        write(&other, "");
        let artifacts = vec![Artifact {
            manifest: manifest.clone(),
            target: "app".into(),
            kind: "bin".into(),
            test: false,
            executable: Some(other.clone()),
        }];
        start.args = Some(vec!["--quiet".into()]);
        let l = start.binary_launch(&artifacts).unwrap();
        assert_eq!(
            (l.program.clone(), l.args.clone()),
            (other, vec!["--quiet".to_owned()])
        );
        // Release: target/release.
        start.release = true;
        assert!(start.binary_launch(&[]).unwrap_err().contains("--release"));
        // The test executable from cargo test --no-run's messages, with the filter and --nocapture.
        let unit = root.join("target/debug/deps/app-77b1");
        let tests = vec![Artifact {
            manifest: manifest.clone(),
            target: "app".into(),
            kind: "bin".into(),
            test: true,
            executable: Some(unit.clone()),
        }];
        start.args = Some(vec!["my_test".into()]);
        start.test = true;
        let l = start.test_launch(&tests).unwrap();
        assert_eq!(
            (l.program.clone(), l.test, l.target.clone()),
            (unit, true, "app".to_owned())
        );
        assert_eq!(l.args, ["my_test", "--nocapture"]);
        assert_eq!(l.env["MODE"], "dev");
    }

    /// The real toolchain of this repository: its sysroot holds the formatters (rust-lldb's `lldb_lookup.py`).
    #[test]
    fn this_repositorys_toolchain_has_the_formatters() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let Ok(sysroot) = sysroot(Path::new("rustc"), &root) else {
            eprintln!("skipped: no rustc");
            return;
        };
        let c = rust_init_commands(&formatters_dir(&sysroot));
        eprintln!("{}: {c:?}", sysroot.display());
        assert!(c.iter().any(|l| l.contains("lldb_lookup.py")), "{c:?}");
    }
}
