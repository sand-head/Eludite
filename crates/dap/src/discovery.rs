//! Locating the debug adapters and the runtimes they need; each is located on the machine, never vendored into git.
//!
//! netcoredbg (MIT, https://github.com/Samsung/netcoredbg; `tools/netcoredbg/fetch.sh` downloads a pinned release),
//! [`AdapterSearch`]:
//!
//! 1. bundled beside the Eludite executable: `<exe dir>/netcoredbg/netcoredbg` (the release archive's folder), then
//!    `<exe dir>/netcoredbg`;
//! 2. `ELUDITE_NETCOREDBG`: the executable, or the folder holding it;
//! 3. `PATH`.
//!
//! The executable is `netcoredbg.exe` on Windows.
//!
//! Mono (brief 0022; the order brief 0003 fixed in `dotnet/src/Eludite.Host/Legacy/MonoInstallation.cs`),
//! [`MonoSearch`]: the setting `debugger.monoPrefix` (`ELUDITE_MONO_PREFIX`), `mono` on `PATH` (its prefix is two
//! folders up from the resolved executable), `~/.local/opt/mono-root/usr`, `/usr`, `/usr/local`,
//! `/Library/Frameworks/Mono.framework/Versions/Current`; a prefix counts when `<prefix>/bin/mono` exists. A Mono
//! not installed at `/usr` runs with brief 0003's environment ([`MonoInstall::env`]).
//!
//! `eludite-dbg-mono` (debuggers/mono, built by `dotnet build dotnet/Eludite.slnx`), [`MonoAdapterSearch`]: beside the
//! Eludite executable (`<exe dir>/eludite-dbg-mono/eludite-dbg-mono.exe`, then `<exe dir>/eludite-dbg-mono.exe`), then
//! the setting `debugger.monoAdapterPath` (`ELUDITE_DBG_MONO`, the file or its folder). It runs as
//! `mono eludite-dbg-mono.exe` ([`MonoInstall::adapter_transport`]).
//!
//! lldb-dap (LLVM, Apache-2.0 WITH LLVM-exception; brief 0029, `protocol/schemas/dap-lldb.md`), [`LldbSearch`]: the
//! setting `debugger.lldbDapPath` (`ELUDITE_LLDB_DAP`, the file or its folder), `lldb-dap` then `lldb-dap-22` down to
//! `lldb-dap-15` on `PATH`, `/usr/lib/llvm-22/bin/lldb-dap` down to `llvm-15`, `xcrun --find lldb-dap` on macOS, then
//! CodeLLDB's `adapter/codelldb` (MIT) under `ELUDITE_CODELLDB` or beside the Eludite executable. The version for
//! messages comes from `lldb-dap --version`, else from `lldb --version` beside it ([`LldbAdapter::version`]).

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::AdapterTransport;

/// The environment variable naming netcoredbg.
pub const ENV_VAR: &str = "ELUDITE_NETCOREDBG";

/// netcoredbg's executable name on this OS.
pub fn netcoredbg_name() -> &'static str {
    if cfg!(windows) {
        "netcoredbg.exe"
    } else {
        "netcoredbg"
    }
}

/// Where netcoredbg was found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FoundIn {
    Bundled,
    Env,
    Path,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Found {
    pub path: PathBuf,
    pub source: FoundIn,
}

impl Found {
    /// The stdio transport that runs it as a DAP server (`--interpreter=vscode`).
    pub fn transport(&self) -> AdapterTransport {
        AdapterTransport::Stdio {
            command: self.path.to_string_lossy().into_owned(),
            args: vec!["--interpreter=vscode".into()],
        }
    }
}

/// The inputs of the search, so tests do not depend on the machine.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AdapterSearch {
    pub exe_dir: Option<PathBuf>,
    pub env: Option<OsString>,
    pub path: Option<OsString>,
}

impl AdapterSearch {
    pub fn from_env() -> Self {
        Self {
            exe_dir: std::env::current_exe()
                .ok()
                .and_then(|p| p.parent().map(Path::to_path_buf)),
            env: std::env::var_os(ENV_VAR).filter(|v| !v.is_empty()),
            path: std::env::var_os("PATH"),
        }
    }

    /// Find netcoredbg, or say where it was looked for.
    pub fn find_netcoredbg(&self) -> Result<Found, String> {
        let name = netcoredbg_name();
        let is_file = |p: &Path| p.is_file();
        if let Some(dir) = &self.exe_dir {
            for p in [dir.join("netcoredbg").join(name), dir.join(name)] {
                if is_file(&p) {
                    return Ok(Found {
                        path: p,
                        source: FoundIn::Bundled,
                    });
                }
            }
        }
        if let Some(env) = &self.env {
            let p = PathBuf::from(env);
            let candidate = if p.is_dir() { p.join(name) } else { p };
            if is_file(&candidate) {
                return Ok(Found {
                    path: candidate,
                    source: FoundIn::Env,
                });
            }
            return Err(format!(
                "{ENV_VAR} is set to {}, which is not netcoredbg",
                candidate.display()
            ));
        }
        if let Some(path) = &self.path {
            for dir in std::env::split_paths(path) {
                let p = dir.join(name);
                if is_file(&p) {
                    return Ok(Found {
                        path: p,
                        source: FoundIn::Path,
                    });
                }
            }
        }
        Err(format!(
            "netcoredbg was not found beside Eludite, in {ENV_VAR} or on PATH; run tools/netcoredbg/fetch.sh and set \
             {ENV_VAR} to the path it prints"
        ))
    }
}

/// The environment variable naming Mono's prefix (the setting `debugger.monoPrefix`).
pub const MONO_PREFIX_ENV: &str = "ELUDITE_MONO_PREFIX";
/// The environment variable naming `eludite-dbg-mono.exe` (the setting `debugger.monoAdapterPath`).
pub const MONO_ADAPTER_ENV: &str = "ELUDITE_DBG_MONO";
/// The Mono adapter's file name.
pub const MONO_ADAPTER: &str = "eludite-dbg-mono.exe";
/// Where `dotnet build dotnet/Eludite.slnx` writes the Mono adapter, from the repository root.
pub const MONO_ADAPTER_BUILT: &str =
    "debuggers/mono/Eludite.Debugger.Mono/bin/Debug/net472/eludite-dbg-mono.exe";

/// The system prefixes, searched last, in order (after `~/.local/opt/mono-root/usr`).
pub const MONO_SYSTEM_PREFIXES: [&str; 3] = [
    "/usr",
    "/usr/local",
    "/Library/Frameworks/Mono.framework/Versions/Current",
];

fn mono_name() -> &'static str {
    if cfg!(windows) { "mono.exe" } else { "mono" }
}

/// Where Mono was found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MonoSource {
    /// The setting `debugger.monoPrefix` (or `ELUDITE_MONO_PREFIX`).
    Configured,
    /// `mono` on `PATH`.
    Path,
    /// `~/.local/opt/mono-root/usr` (brief 0003's user-space Mono).
    UserSpace,
    /// `/usr`, `/usr/local` or the macOS framework.
    System,
}

/// A located Mono.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MonoInstall {
    pub prefix: PathBuf,
    /// `<prefix>/bin/mono`.
    pub mono: PathBuf,
    pub source: MonoSource,
    /// Variables to set when running it: `PATH` with `<prefix>/bin` first, and for a prefix other than `/usr`
    /// `LD_LIBRARY_PATH`, `MONO_CFG_DIR` and `MONO_GAC_PREFIX` (brief 0003's relocated Mono).
    pub env: Vec<(String, String)>,
}

impl MonoInstall {
    /// The stdio transport that runs `adapter` (`eludite-dbg-mono.exe`) under this Mono.
    pub fn adapter_transport(&self, adapter: &Path) -> AdapterTransport {
        AdapterTransport::Stdio {
            command: self.mono.to_string_lossy().into_owned(),
            args: vec![adapter.to_string_lossy().into_owned()],
        }
    }

    /// The version from the first line of `mono --version` (`6.8.0.105`). Runs Mono: call it off the UI thread.
    pub fn version(&self) -> Option<String> {
        let out = Command::new(&self.mono)
            .arg("--version")
            .envs(self.env.iter().map(|(k, v)| (k, v)))
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output()
            .ok()?;
        parse_mono_version(&String::from_utf8_lossy(&out.stdout))
    }
}

/// The version in `mono --version`'s first line: `Mono JIT compiler version 6.8.0.105 (Debian ...)` gives `6.8.0.105`.
pub fn parse_mono_version(output: &str) -> Option<String> {
    let first = output.lines().next()?;
    let after = &first[first.find("version ")? + "version ".len()..];
    let v = after.split_whitespace().next()?;
    (!v.is_empty()).then(|| v.to_owned())
}

/// The variables a Mono at `prefix` runs with, given the current `PATH` and `LD_LIBRARY_PATH`.
pub fn mono_env(
    prefix: &Path,
    path: Option<&OsString>,
    ld: Option<&OsString>,
) -> Vec<(String, String)> {
    let bin = prefix.join("bin");
    let sep = if cfg!(windows) { ";" } else { ":" };
    let current = path
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_default();
    let has_bin = std::env::split_paths(&OsString::from(&current)).any(|d| d == bin);
    let path = if has_bin {
        current
    } else if current.is_empty() {
        bin.to_string_lossy().into_owned()
    } else {
        format!("{}{sep}{current}", bin.display())
    };
    let mut env = vec![("PATH".to_owned(), path)];
    let normalized = prefix.to_string_lossy().trim_end_matches('/').to_owned();
    if normalized != "/usr" {
        let lib = prefix.join("lib").to_string_lossy().into_owned();
        let ld = ld
            .map(|l| l.to_string_lossy().into_owned())
            .filter(|l| !l.is_empty());
        env.push((
            "LD_LIBRARY_PATH".to_owned(),
            match ld {
                Some(l) => format!("{lib}{sep}{l}"),
                None => lib,
            },
        ));
        let etc = prefix
            .parent()
            .map(|p| p.join("etc"))
            .unwrap_or_else(|| prefix.join("etc"));
        env.push((
            "MONO_CFG_DIR".to_owned(),
            etc.to_string_lossy().into_owned(),
        ));
        env.push((
            "MONO_GAC_PREFIX".to_owned(),
            prefix.to_string_lossy().into_owned(),
        ));
    }
    env
}

/// The inputs of the Mono search, so tests do not depend on the machine.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MonoSearch {
    /// The setting `debugger.monoPrefix` (the store resolves `ELUDITE_MONO_PREFIX`).
    pub configured: Option<PathBuf>,
    pub path: Option<OsString>,
    pub home: Option<PathBuf>,
    pub ld_library_path: Option<OsString>,
    /// The system prefixes (`/usr`, `/usr/local`, the macOS framework); tests replace them.
    pub system: Vec<PathBuf>,
}

impl MonoSearch {
    /// The machine's `PATH`, home and system prefixes, without a configured prefix (the settings store gives that).
    pub fn from_env() -> Self {
        Self {
            configured: None,
            path: std::env::var_os("PATH"),
            home: std::env::var_os("HOME")
                .or_else(|| std::env::var_os("USERPROFILE"))
                .map(PathBuf::from),
            ld_library_path: std::env::var_os("LD_LIBRARY_PATH"),
            system: MONO_SYSTEM_PREFIXES.iter().map(PathBuf::from).collect(),
        }
    }

    /// The prefixes to try, in order, with where each came from.
    fn candidates(&self) -> Vec<(PathBuf, MonoSource)> {
        let mut c = Vec::new();
        if let Some(p) = &self.configured {
            c.push((p.clone(), MonoSource::Configured));
        }
        if let Some(path) = &self.path {
            for dir in std::env::split_paths(path) {
                let exe = dir.join(mono_name());
                if exe.is_file() {
                    let real = std::fs::canonicalize(&exe).unwrap_or(exe);
                    if let Some(prefix) = real.parent().and_then(Path::parent) {
                        c.push((prefix.to_path_buf(), MonoSource::Path));
                    }
                }
            }
        }
        if let Some(home) = &self.home {
            c.push((home.join(".local/opt/mono-root/usr"), MonoSource::UserSpace));
        }
        c.extend(self.system.iter().map(|p| (p.clone(), MonoSource::System)));
        c
    }

    /// Find Mono, or say where it was looked for and how to install it.
    pub fn find_mono(&self) -> Result<MonoInstall, String> {
        for (prefix, source) in self.candidates() {
            let mono = prefix.join("bin").join(mono_name());
            if mono.is_file() {
                let env = mono_env(&prefix, self.path.as_ref(), self.ld_library_path.as_ref());
                return Ok(MonoInstall {
                    prefix,
                    mono,
                    source,
                    env,
                });
            }
        }
        let configured = self
            .configured
            .as_ref()
            .map(|p| {
                format!(
                    "debugger.monoPrefix ({}, which has no bin/mono), ",
                    p.display()
                )
            })
            .unwrap_or_else(|| format!("debugger.monoPrefix ({MONO_PREFIX_ENV}, not set), "));
        Err(format!(
            "Mono was not found: searched {configured}mono on PATH, ~/.local/opt/mono-root/usr, {}. Install it \
             (mono-devel on Debian and Ubuntu, mono on Arch, the Mono framework on macOS) or set debugger.monoPrefix \
             to its prefix.",
            self.system
                .iter()
                .map(|p| p.display().to_string())
                .collect::<Vec<_>>()
                .join(", ")
        ))
    }
}

/// The inputs of the `eludite-dbg-mono` search.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MonoAdapterSearch {
    pub exe_dir: Option<PathBuf>,
    /// The setting `debugger.monoAdapterPath` (the store resolves `ELUDITE_DBG_MONO`): the file or its folder.
    pub configured: Option<PathBuf>,
}

impl MonoAdapterSearch {
    pub fn from_env() -> Self {
        Self {
            exe_dir: std::env::current_exe()
                .ok()
                .and_then(|p| p.parent().map(Path::to_path_buf)),
            configured: None,
        }
    }

    /// Find `eludite-dbg-mono.exe`, or name both places and how to build it.
    pub fn find(&self) -> Result<PathBuf, String> {
        let beside = self.exe_dir.as_ref().map(|dir| {
            [
                dir.join("eludite-dbg-mono").join(MONO_ADAPTER),
                dir.join(MONO_ADAPTER),
            ]
        });
        if let Some(found) = beside.iter().flatten().find(|p| p.is_file()) {
            return Ok(found.clone());
        }
        if let Some(c) = &self.configured {
            let candidate = if c.is_dir() {
                c.join(MONO_ADAPTER)
            } else {
                c.clone()
            };
            if candidate.is_file() {
                return Ok(candidate);
            }
            return Err(format!(
                "debugger.monoAdapterPath ({MONO_ADAPTER_ENV}) is {}, which is not {MONO_ADAPTER}; build it with \
                 `dotnet build dotnet/Eludite.slnx` (it writes {MONO_ADAPTER_BUILT})",
                candidate.display()
            ));
        }
        let beside = beside
            .map(|[a, b]| format!("{} or {}", a.display(), b.display()))
            .unwrap_or_else(|| "beside the eludite executable".to_owned());
        Err(format!(
            "{MONO_ADAPTER} was not found beside Eludite ({beside}) or at the setting debugger.monoAdapterPath \
             ({MONO_ADAPTER_ENV}); build it with `dotnet build dotnet/Eludite.slnx`, which writes {MONO_ADAPTER_BUILT}, \
             and set debugger.monoAdapterPath to that file"
        ))
    }
}

/// The environment variable naming lldb-dap (the setting `debugger.lldbDapPath`).
pub const LLDB_DAP_ENV: &str = "ELUDITE_LLDB_DAP";
/// The environment variable naming a CodeLLDB installation (its extension folder, `adapter` folder or executable).
pub const CODELLDB_ENV: &str = "ELUDITE_CODELLDB";
/// The versioned names searched on `PATH` and under `/usr/lib/llvm-NN`, newest first.
pub const LLDB_VERSIONS: std::ops::RangeInclusive<u32> = 15..=22;

fn exe(name: &str) -> String {
    format!("{name}{}", std::env::consts::EXE_SUFFIX)
}

/// Which LLDB adapter was found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LldbFlavor {
    /// LLVM's `lldb-dap` (formerly `lldb-vscode`).
    LldbDap,
    /// CodeLLDB's `adapter/codelldb`.
    CodeLldb,
}

/// Where the LLDB adapter was found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LldbSource {
    /// The setting `debugger.lldbDapPath` (or `ELUDITE_LLDB_DAP`).
    Configured,
    /// `lldb-dap` or `lldb-dap-NN` on `PATH`.
    Path,
    /// `/usr/lib/llvm-NN/bin/lldb-dap`.
    Llvm,
    /// `xcrun --find lldb-dap` (macOS).
    Xcrun,
    /// CodeLLDB under `ELUDITE_CODELLDB` or beside the executable.
    CodeLldb,
}

/// A located LLDB debug adapter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LldbAdapter {
    pub path: PathBuf,
    pub flavor: LldbFlavor,
    pub source: LldbSource,
}

impl LldbAdapter {
    /// An adapter at `path`: CodeLLDB when the file is named `codelldb`, else lldb-dap.
    pub fn at(path: PathBuf, source: LldbSource) -> Self {
        let codelldb = path
            .file_stem()
            .is_some_and(|n| n.to_string_lossy().eq_ignore_ascii_case("codelldb"));
        Self {
            path,
            flavor: if codelldb {
                LldbFlavor::CodeLldb
            } else {
                LldbFlavor::LldbDap
            },
            source,
        }
    }

    /// The stdio transport: both adapters speak DAP on stdin and stdout when started without arguments.
    pub fn transport(&self) -> AdapterTransport {
        AdapterTransport::Stdio {
            command: self.path.to_string_lossy().into_owned(),
            args: Vec::new(),
        }
    }

    /// The adapter's name for messages: `lldb-dap` or `codelldb`.
    pub fn name(&self) -> &'static str {
        match self.flavor {
            LldbFlavor::LldbDap => "lldb-dap",
            LldbFlavor::CodeLldb => "codelldb",
        }
    }

    /// The version, read once (it runs the adapter and maybe `lldb`: call it off the UI thread): `lldb-dap --version`,
    /// else `lldb --version` beside the resolved executable (lldb-dap 18 prints nothing for `--version`). `None` when
    /// neither says.
    pub fn version(&self) -> Option<String> {
        if let Some(v) =
            command_output(&self.path, &["--version"]).and_then(|o| parse_lldb_version(&o))
        {
            return Some(v);
        }
        let real = std::fs::canonicalize(&self.path).unwrap_or_else(|_| self.path.clone());
        let dir = real.parent()?;
        let lldb = dir.join(exe("lldb"));
        command_output(&lldb, &["--version"]).and_then(|o| parse_lldb_version(&o))
    }

    /// `session.adapter`: `lldb-dap 18.1.3 (stdio)`, `codelldb (stdio)`, `lldb-dap (unknown version) (stdio)`.
    pub fn describe(&self, version: Option<&str>) -> String {
        match (self.flavor, version) {
            (_, Some(v)) => format!("{} {v} (stdio)", self.name()),
            (LldbFlavor::CodeLldb, None) => "codelldb (stdio)".to_owned(),
            (LldbFlavor::LldbDap, None) => "lldb-dap (unknown version) (stdio)".to_owned(),
        }
    }
}

/// Run `program args` with stdin closed and return its stdout and stderr, killing it after ten seconds (a loaded
/// machine took more than two to start `lldb --version`).
fn command_output(program: &Path, args: &[&str]) -> Option<String> {
    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .ok()?;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if std::time::Instant::now() < deadline => {
                std::thread::sleep(std::time::Duration::from_millis(5))
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
    let out = child.wait_with_output().ok()?;
    let mut text = String::from_utf8_lossy(&out.stdout).into_owned();
    text.push_str(&String::from_utf8_lossy(&out.stderr));
    Some(text)
}

/// The version in `lldb version 18.1.3` or `lldb-dap version 19.1.0` (also with a vendor prefix such as
/// `Apple lldb-1600.0.36.3` or `Ubuntu LLDB 18.1.3`): the first word after `version`, or after `lldb-`.
pub fn parse_lldb_version(output: &str) -> Option<String> {
    output.lines().find_map(|line| {
        let lower = line.to_ascii_lowercase();
        if !lower.contains("lldb") {
            return None;
        }
        let after = if let Some(i) = lower.find("version ") {
            &line[i + "version ".len()..]
        } else {
            let i = lower.find("lldb-")?;
            &line[i + "lldb-".len()..]
        };
        let v = after
            .split_whitespace()
            .next()?
            .trim_matches(|c: char| c == ',' || c == ')');
        (v.chars().next().is_some_and(|c| c.is_ascii_digit())).then(|| v.to_owned())
    })
}

/// The package that installs lldb-dap on `platform` (from `tools/lldb-dap/README.md`).
pub fn lldb_install_hint(platform: crate::launch::Platform) -> &'static str {
    match platform {
        crate::launch::Platform::Linux => {
            "install it: `sudo apt install lldb-18` on Debian and Ubuntu (lldb-dap-18), `sudo dnf install lldb` on \
             Fedora, `sudo pacman -S lldb` on Arch"
        }
        crate::launch::Platform::MacOs => {
            "install Xcode or its Command Line Tools (`xcode-select --install`), which carry lldb-dap"
        }
        crate::launch::Platform::Windows => {
            "install LLVM with its Windows installer (https://github.com/llvm/llvm-project/releases), which carries \
             lldb-dap.exe"
        }
    }
}

/// The inputs of the lldb-dap search, so tests do not depend on the machine.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LldbSearch {
    /// The setting `debugger.lldbDapPath` (the store resolves `ELUDITE_LLDB_DAP`): the executable or its folder.
    pub configured: Option<PathBuf>,
    pub path: Option<OsString>,
    /// The folder holding the `llvm-NN` folders (`/usr/lib`); tests replace it.
    pub llvm_root: Option<PathBuf>,
    /// Ask `xcrun --find lldb-dap` (macOS).
    pub xcrun: bool,
    /// `ELUDITE_CODELLDB`.
    pub codelldb: Option<PathBuf>,
    pub exe_dir: Option<PathBuf>,
}

impl LldbSearch {
    /// The machine's `PATH`, `/usr/lib`, `xcrun` on macOS, `ELUDITE_CODELLDB` and the executable's folder, without a
    /// configured path (the settings store gives that).
    pub fn from_env() -> Self {
        Self {
            configured: None,
            path: std::env::var_os("PATH"),
            llvm_root: (!cfg!(windows)).then(|| PathBuf::from("/usr/lib")),
            xcrun: cfg!(target_os = "macos"),
            codelldb: std::env::var_os(CODELLDB_ENV)
                .filter(|v| !v.is_empty())
                .map(PathBuf::from),
            exe_dir: std::env::current_exe()
                .ok()
                .and_then(|p| p.parent().map(Path::to_path_buf)),
        }
    }

    /// The names tried on `PATH`, in order: `lldb-dap`, then `lldb-dap-22` down to `lldb-dap-15`.
    pub fn path_names() -> Vec<String> {
        std::iter::once(exe("lldb-dap"))
            .chain(LLDB_VERSIONS.rev().map(|v| exe(&format!("lldb-dap-{v}"))))
            .collect()
    }

    /// Find the adapter, or say where it was looked for and what to install on `platform`.
    pub fn find(&self, platform: crate::launch::Platform) -> Result<LldbAdapter, String> {
        if let Some(c) = &self.configured {
            let candidate = if c.is_dir() {
                [exe("lldb-dap"), exe("codelldb")]
                    .iter()
                    .map(|n| c.join(n))
                    .find(|p| p.is_file())
                    .unwrap_or_else(|| c.join(exe("lldb-dap")))
            } else {
                c.clone()
            };
            if candidate.is_file() {
                return Ok(LldbAdapter::at(candidate, LldbSource::Configured));
            }
            return Err(format!(
                "debugger.lldbDapPath ({LLDB_DAP_ENV}) is {}, which is not lldb-dap; {}",
                candidate.display(),
                lldb_install_hint(platform)
            ));
        }
        if let Some(path) = &self.path {
            let dirs: Vec<PathBuf> = std::env::split_paths(path).collect();
            for name in Self::path_names() {
                if let Some(p) = dirs.iter().map(|d| d.join(&name)).find(|p| p.is_file()) {
                    return Ok(LldbAdapter::at(p, LldbSource::Path));
                }
            }
        }
        if let Some(root) = &self.llvm_root {
            for v in LLDB_VERSIONS.rev() {
                let p = root
                    .join(format!("llvm-{v}"))
                    .join("bin")
                    .join(exe("lldb-dap"));
                if p.is_file() {
                    return Ok(LldbAdapter::at(p, LldbSource::Llvm));
                }
            }
        }
        if self.xcrun
            && let Some(out) = command_output(Path::new("xcrun"), &["--find", "lldb-dap"])
        {
            let p = PathBuf::from(out.lines().next().unwrap_or_default().trim());
            if p.is_absolute() && p.is_file() {
                return Ok(LldbAdapter::at(p, LldbSource::Xcrun));
            }
        }
        let codelldb_in = |dir: &Path| {
            [
                dir.to_path_buf(),
                dir.join(exe("codelldb")),
                dir.join("adapter").join(exe("codelldb")),
            ]
            .into_iter()
            .find(|p| p.is_file())
        };
        if let Some(found) = self.codelldb.as_deref().and_then(codelldb_in) {
            return Ok(LldbAdapter::at(found, LldbSource::CodeLldb));
        }
        if let Some(found) = self
            .exe_dir
            .as_ref()
            .and_then(|d| codelldb_in(&d.join("codelldb")))
        {
            return Ok(LldbAdapter::at(found, LldbSource::CodeLldb));
        }
        let llvm = self
            .llvm_root
            .as_ref()
            .map(|r| format!(", {}/llvm-22/bin/lldb-dap down to llvm-15", r.display()))
            .unwrap_or_default();
        Err(format!(
            "lldb-dap was not found: searched debugger.lldbDapPath ({LLDB_DAP_ENV}, not set), lldb-dap and lldb-dap-22 \
             down to lldb-dap-15 on PATH{llvm}{}, CodeLLDB ({CODELLDB_ENV}, or codelldb/adapter/codelldb beside \
             Eludite). To debug Rust, {} (tools/lldb-dap/README.md), or set debugger.lldbDapPath",
            if self.xcrun {
                ", xcrun --find lldb-dap"
            } else {
                ""
            },
            lldb_install_hint(platform)
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::launch::Platform;

    fn touch(p: &Path) {
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, b"").unwrap();
    }

    #[test]
    fn search_order_is_bundled_env_path() {
        let t = tempfile::tempdir().unwrap();
        let name = netcoredbg_name();
        let exe = t.path().join("bin");
        let env_dir = t.path().join("env");
        let on_path = t.path().join("path");
        touch(&env_dir.join(name));
        touch(&on_path.join(name));
        let path = std::env::join_paths([t.path().join("nothing"), on_path.clone()]).unwrap();
        let mut s = AdapterSearch {
            exe_dir: Some(exe.clone()),
            env: None,
            path: Some(path),
        };
        assert_eq!(s.find_netcoredbg().unwrap().source, FoundIn::Path);
        // A folder in the variable means the executable inside it.
        s.env = Some(env_dir.clone().into());
        let found = s.find_netcoredbg().unwrap();
        assert_eq!(
            (found.source, found.path),
            (FoundIn::Env, env_dir.join(name))
        );
        touch(&exe.join("netcoredbg").join(name));
        let found = s.find_netcoredbg().unwrap();
        assert_eq!(found.source, FoundIn::Bundled);
        assert_eq!(found.path, exe.join("netcoredbg").join(name));
        assert_eq!(
            found.transport(),
            AdapterTransport::Stdio {
                command: found.path.to_string_lossy().into_owned(),
                args: vec!["--interpreter=vscode".into()]
            }
        );
        // A wrong variable is an error, not a silent fallback to PATH.
        let wrong = AdapterSearch {
            exe_dir: None,
            env: Some(t.path().join("missing").into()),
            path: s.path.clone(),
        };
        assert!(wrong.find_netcoredbg().unwrap_err().contains(ENV_VAR));
        let none = AdapterSearch::default();
        assert!(none.find_netcoredbg().unwrap_err().contains("fetch.sh"));
    }

    #[test]
    fn mono_search_order_is_setting_path_user_space_system() {
        let t = tempfile::tempdir().unwrap();
        let mk = |p: &Path| touch(&p.join("bin").join(mono_name()));
        let configured = t.path().join("configured");
        let on_path = t.path().join("onpath");
        let home = t.path().join("home");
        let system = t.path().join("sys");
        mk(&system);
        let path = std::env::join_paths([t.path().join("nothing"), on_path.join("bin")]).unwrap();
        let mut s = MonoSearch {
            configured: None,
            path: Some(path),
            home: Some(home.clone()),
            ld_library_path: None,
            system: vec![t.path().join("nosys"), system.clone()],
        };
        let found = s.find_mono().unwrap();
        assert_eq!(
            (found.source, found.prefix.clone()),
            (MonoSource::System, system.clone())
        );
        let user = home.join(".local/opt/mono-root/usr");
        mk(&user);
        assert_eq!(s.find_mono().unwrap().source, MonoSource::UserSpace);
        mk(&on_path);
        let found = s.find_mono().unwrap();
        assert_eq!(found.source, MonoSource::Path);
        // Two folders up from the resolved executable.
        assert_eq!(found.prefix, std::fs::canonicalize(&on_path).unwrap());
        // A configured prefix without bin/mono falls through; with one it wins.
        s.configured = Some(configured.clone());
        assert_eq!(s.find_mono().unwrap().source, MonoSource::Path);
        mk(&configured);
        let found = s.find_mono().unwrap();
        assert_eq!(
            (found.source, found.mono.clone()),
            (
                MonoSource::Configured,
                configured.join("bin").join(mono_name())
            )
        );
        // A relocated prefix runs with brief 0003's environment.
        let env: std::collections::HashMap<_, _> = found.env.iter().cloned().collect();
        assert!(env["PATH"].starts_with(&configured.join("bin").to_string_lossy().into_owned()));
        assert_eq!(
            env["LD_LIBRARY_PATH"],
            configured.join("lib").to_string_lossy()
        );
        assert_eq!(env["MONO_GAC_PREFIX"], configured.to_string_lossy());
        assert_eq!(env["MONO_CFG_DIR"], t.path().join("etc").to_string_lossy());
        assert_eq!(
            found.adapter_transport(Path::new("/a/eludite-dbg-mono.exe")),
            AdapterTransport::Stdio {
                command: found.mono.to_string_lossy().into_owned(),
                args: vec!["/a/eludite-dbg-mono.exe".into()]
            }
        );
        // /usr needs only PATH (a Unix layout: Mono debugging is for Linux and macOS).
        if cfg!(unix) {
            let usr = mono_env(
                Path::new("/usr"),
                Some(&OsString::from("/usr/bin:/bin")),
                None,
            );
            assert_eq!(usr, vec![("PATH".to_owned(), "/usr/bin:/bin".to_owned())]);
        }

        let none = MonoSearch::default();
        let err = none.find_mono().unwrap_err();
        assert!(
            err.contains("mono-devel") && err.contains("debugger.monoPrefix"),
            "{err}"
        );
        assert_eq!(
            parse_mono_version(
                "Mono JIT compiler version 6.8.0.105 (Debian 6.8.0.105+dfsg-3.6 Wed Jul 31 2024)\nCopyright"
            )
            .as_deref(),
            Some("6.8.0.105")
        );
        assert_eq!(parse_mono_version("nonsense"), None);
    }

    #[test]
    fn mono_adapter_search_is_beside_then_setting() {
        let t = tempfile::tempdir().unwrap();
        let exe = t.path().join("bin");
        let built = t.path().join("built");
        touch(&built.join(MONO_ADAPTER));
        let mut s = MonoAdapterSearch {
            exe_dir: Some(exe.clone()),
            configured: None,
        };
        let err = s.find().unwrap_err();
        assert!(
            err.contains(
                &exe.join("eludite-dbg-mono")
                    .join(MONO_ADAPTER)
                    .display()
                    .to_string()
            ) && err.contains(MONO_ADAPTER_ENV)
                && err.contains("dotnet build dotnet/Eludite.slnx")
                && err.contains(MONO_ADAPTER_BUILT),
            "{err}"
        );
        // The setting: a folder means the adapter inside it.
        s.configured = Some(built.clone());
        assert_eq!(s.find().unwrap(), built.join(MONO_ADAPTER));
        s.configured = Some(t.path().join("missing.exe"));
        assert!(s.find().unwrap_err().contains("missing.exe"));
        // Beside the executable wins: the folder first, then the file.
        touch(&exe.join(MONO_ADAPTER));
        assert_eq!(s.find().unwrap(), exe.join(MONO_ADAPTER));
        touch(&exe.join("eludite-dbg-mono").join(MONO_ADAPTER));
        assert_eq!(
            s.find().unwrap(),
            exe.join("eludite-dbg-mono").join(MONO_ADAPTER)
        );
    }

    #[test]
    fn lldb_dap_search_order_is_setting_path_versioned_llvm_codelldb() {
        let t = tempfile::tempdir().unwrap();
        let name = |n: &str| exe(n);
        let path_dir = t.path().join("path");
        let llvm = t.path().join("lib");
        let mut s = LldbSearch {
            configured: None,
            path: Some(std::env::join_paths([t.path().join("nothing"), path_dir.clone()]).unwrap()),
            llvm_root: Some(llvm.clone()),
            xcrun: false,
            codelldb: None,
            exe_dir: Some(t.path().join("bin")),
        };
        // Nothing yet: the message lists the places and the package for the platform.
        let err = s.find(Platform::Linux).unwrap_err();
        assert!(
            err.contains("lldb-dap-22 down to lldb-dap-15 on PATH")
                && err.contains("llvm-22/bin/lldb-dap")
                && err.contains("apt install lldb-18")
                && err.contains(LLDB_DAP_ENV)
                && err.contains(CODELLDB_ENV),
            "{err}"
        );
        assert!(s.find(Platform::MacOs).unwrap_err().contains("Xcode"));
        assert!(s.find(Platform::Windows).unwrap_err().contains("LLVM"));
        // CodeLLDB beside the executable, then under ELUDITE_CODELLDB (an extension folder with `adapter/`).
        touch(&t.path().join("bin/codelldb/adapter").join(name("codelldb")));
        let found = s.find(Platform::Linux).unwrap();
        assert_eq!(
            (found.source, found.flavor),
            (LldbSource::CodeLldb, LldbFlavor::CodeLldb)
        );
        let ext = t.path().join("ext");
        touch(&ext.join("adapter").join(name("codelldb")));
        s.codelldb = Some(ext.clone());
        assert_eq!(
            s.find(Platform::Linux).unwrap().path,
            ext.join("adapter").join(name("codelldb"))
        );
        // /usr/lib/llvm-NN, newest first.
        touch(&llvm.join("llvm-16/bin").join(name("lldb-dap")));
        touch(&llvm.join("llvm-18/bin").join(name("lldb-dap")));
        let found = s.find(Platform::Linux).unwrap();
        assert_eq!(
            (found.source, found.flavor, found.path.clone()),
            (
                LldbSource::Llvm,
                LldbFlavor::LldbDap,
                llvm.join("llvm-18/bin").join(name("lldb-dap"))
            )
        );
        // The versioned names on PATH, newest first, then the plain name before them.
        touch(&path_dir.join(name("lldb-dap-17")));
        touch(&path_dir.join(name("lldb-dap-19")));
        assert_eq!(
            s.find(Platform::Linux).unwrap().path,
            path_dir.join(name("lldb-dap-19"))
        );
        touch(&path_dir.join(name("lldb-dap")));
        let found = s.find(Platform::Linux).unwrap();
        assert_eq!(
            (found.source, found.path.clone()),
            (LldbSource::Path, path_dir.join(name("lldb-dap")))
        );
        assert_eq!(
            found.transport(),
            AdapterTransport::Stdio {
                command: found.path.to_string_lossy().into_owned(),
                args: vec![]
            }
        );
        // The setting wins: a file, or a folder holding lldb-dap; a wrong one is an error, not a fallback.
        let configured = t.path().join("mine");
        touch(&configured.join(name("lldb-dap")));
        s.configured = Some(configured.clone());
        let found = s.find(Platform::Linux).unwrap();
        assert_eq!(
            (found.source, found.path.clone()),
            (LldbSource::Configured, configured.join(name("lldb-dap")))
        );
        s.configured = Some(t.path().join("missing"));
        assert!(s.find(Platform::Linux).unwrap_err().contains("missing"));
        assert_eq!(LldbSearch::path_names()[1], name("lldb-dap-22"));
        assert_eq!(
            LldbSearch::path_names().last().unwrap(),
            &name("lldb-dap-15")
        );
    }

    #[test]
    fn lldb_versions_parse_and_describe() {
        assert_eq!(
            parse_lldb_version("lldb version 18.1.3").as_deref(),
            Some("18.1.3")
        );
        assert_eq!(
            parse_lldb_version(
                "Ubuntu LLDB version 19.1.1 (https://github.com/llvm/llvm-project ...)"
            )
            .as_deref(),
            Some("19.1.1")
        );
        assert_eq!(
            parse_lldb_version("lldb-dap version 20.0.0git").as_deref(),
            Some("20.0.0git")
        );
        assert_eq!(
            parse_lldb_version("Apple lldb-1600.0.36.3\nApple Swift version 6").as_deref(),
            Some("1600.0.36.3")
        );
        assert_eq!(parse_lldb_version(""), None);
        assert_eq!(parse_lldb_version("OVERVIEW: LLDB DAP"), None);
        let a = LldbAdapter::at(PathBuf::from("/usr/bin/lldb-dap-18"), LldbSource::Path);
        assert_eq!(a.describe(Some("18.1.3")), "lldb-dap 18.1.3 (stdio)");
        assert_eq!(a.describe(None), "lldb-dap (unknown version) (stdio)");
        let c = LldbAdapter::at(PathBuf::from("/x/adapter/codelldb"), LldbSource::CodeLldb);
        assert_eq!(c.describe(None), "codelldb (stdio)");
    }

    /// The real lldb-dap of this machine, when there is one: its version comes from `lldb --version` beside it.
    #[test]
    fn the_installed_lldb_dap_has_a_version() {
        let Ok(found) = LldbSearch::from_env().find(Platform::current()) else {
            eprintln!("skipped: no lldb-dap on this machine");
            return;
        };
        let v = found.version();
        eprintln!("{} {v:?}", found.path.display());
        if found.flavor == LldbFlavor::LldbDap {
            assert!(v.is_some_and(|v| v.chars().next().is_some_and(|c| c.is_ascii_digit())));
        }
    }
}
