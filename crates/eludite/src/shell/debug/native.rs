//! Debugging Cargo packages (Rust) with lldb-dap (brief 0029, `protocol/schemas/dap-lldb.md`): the startup project may
//! be a member package of the open folder's Cargo workspace, and F5 then builds it through the Cargo build path
//! (brief 0019's `cargo build -p`, the build gate of brief 0020) and debugs its executable under lldb-dap, with the
//! debugger windows and `eludite.debug.*` commands unchanged.
//!
//! - **Which package** ([`resolve_cargo`]): the start's `project` names a member (by package name, `Cargo.toml` or
//!   folder), else the startup project is one (Set as Startup Project, [`Shell::set_cargo_startup`]), else, when no
//!   .NET project is executable, the workspace's root package with a binary, else its first member with one.
//! - **On the launch thread** ([`prepare`], [`connect`]): the launch configuration from `eludite_dap::cargo` (the
//!   binary's executable or, with `test`, `cargo test --no-run`'s test executable, whose output goes to the Debug
//!   source as it builds), `rustc --print sysroot` once for the formatters and `rust-src`, finding lldb-dap and reading
//!   its version. The UI thread does none of it.
//! - **What lldb-dap 18 says differently** ([`adapt`], on the client's reader thread): the stop a pause causes is an
//!   `exception` stop described `signal SIGSTOP` (reported as `pause`); standard library frames carry
//!   `/rustc/<commit>/...` paths, mapped under `rust-src` when it is installed and otherwise shown as external code
//!   (no path, `subtle`).
//! - **Hit counts** ([`adapt_capabilities`]): lldb-dap's `hitCondition` is a bare number with LLDB's ignore-count
//!   meaning, so the shell counts hits itself for `N`, `>=N` and `%N`.
//! - **Rust panics** ([`function_breakpoints`], [`with_rust_panics`]): the Exception Settings row is a function
//!   breakpoint on `rust_panic`, sent before `configurationDone` and again when the row changes during a session, in
//!   one `setFunctionBreakpoints` list after the user's function breakpoints (brief 0026).

use std::path::{Path, PathBuf};

use eludite_commands::CommandError;
use eludite_commands::debug::CargoOptions;
use eludite_commands::project::{ProjectOutput, StartupProjectOutput};
use eludite_dap::cargo::{self, CargoPackageInfo, CargoStart};
use eludite_dap::discovery::LldbSearch;
use eludite_dap::launch::{LaunchConfig, Platform};
use eludite_dap::types::{Capabilities, Event, FunctionBreakpoint, OutputEvent};
use eludite_dap::{ClientEvent, Connection, transport};
use eludite_workspace::cargo::{CargoWorkspace, TargetKind};
use gpui::Context;
use serde_json::Value;

use super::super::Shell;
use super::super::documents::normalize_path;
use super::Connector;

/// The function the Rust panics row breaks in (what `rust-lldb` users set: `b rust_panic`).
pub const RUST_PANIC: &str = "rust_panic";

/// How native sessions find their adapter and the Rust formatters (the settings `debugger.lldbDapPath` and
/// `debugger.rustFormatters`).
#[derive(Debug, Clone)]
pub struct NativeSetup {
    pub lldb: LldbSearch,
    pub formatters: bool,
    /// Use this sysroot instead of running `rustc --print sysroot` (tests).
    pub sysroot: Option<PathBuf>,
}

impl NativeSetup {
    /// The machine's search, the formatters on; the configured adapter comes from the settings store.
    pub fn from_env() -> Self {
        Self {
            lldb: LldbSearch::from_env(),
            formatters: true,
            sysroot: None,
        }
    }
}

/// The open folder's Cargo workspace as the launch thread needs it (a snapshot of the shell's model).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CargoContext {
    pub root: PathBuf,
    pub manifest: PathBuf,
    pub target_dir: PathBuf,
    pub members: Vec<CargoPackageInfo>,
    /// The Release configuration of the toolbar (brief 0019: `--release`, `target/release`).
    pub release: bool,
    /// The cargo builds use (`build.cargoPath`).
    pub cargo: PathBuf,
}

impl CargoContext {
    pub fn from_workspace(ws: &CargoWorkspace, release: bool, cargo: PathBuf) -> Self {
        Self {
            root: ws.root.clone(),
            manifest: ws.manifest.clone(),
            target_dir: ws.target_directory.clone(),
            members: ws
                .members
                .iter()
                .map(|p| CargoPackageInfo {
                    name: p.name.clone(),
                    manifest: p.manifest_path.clone(),
                    bins: p
                        .targets
                        .iter()
                        .filter(|t| t.kind == TargetKind::Bin)
                        .map(|t| t.name.clone())
                        .collect(),
                    has_lib: p
                        .targets
                        .iter()
                        .any(|t| matches!(t.kind, TargetKind::Lib | TargetKind::ProcMacro)),
                })
                .collect(),
            release,
            cargo,
        }
    }

    /// The member whose `Cargo.toml` (or folder) is `path`.
    pub fn member_at(&self, path: &Path) -> Option<&CargoPackageInfo> {
        let wanted = normalize_path(path);
        self.members.iter().find(|m| {
            normalize_path(&m.manifest) == wanted
                || m.manifest.parent().map(normalize_path) == Some(wanted.clone())
        })
    }

    /// The member `hint` names: its package name, or its `Cargo.toml` or folder (absolute, or relative to the root).
    pub fn member(&self, hint: &str) -> Option<&CargoPackageInfo> {
        if let Some(m) = self.members.iter().find(|m| m.name == hint) {
            return Some(m);
        }
        let p = Path::new(hint);
        let abs = if p.is_absolute() {
            p.to_path_buf()
        } else {
            self.root.join(p)
        };
        self.member_at(&abs)
    }

    /// The package F5 starts when nothing is chosen: the root package when it has a binary, else the first member with
    /// one.
    pub fn default_member(&self) -> Option<&CargoPackageInfo> {
        let root = normalize_path(&self.manifest);
        self.members
            .iter()
            .find(|m| normalize_path(&m.manifest) == root && !m.bins.is_empty())
            .or_else(|| self.members.iter().find(|m| !m.bins.is_empty()))
    }
}

/// What the launch thread needs for a native session.
#[derive(Debug, Clone)]
pub struct NativeJob {
    pub setup: NativeSetup,
    pub cargo: Option<CargoContext>,
    pub options: CargoOptions,
    /// The Exception Settings window's Rust panics row.
    pub rust_panics: bool,
}

/// The Cargo package a start runs, if it runs one: the hint's member, else the startup project's, else (when no .NET
/// project is executable: `dotnet_default` false) the default member. `None`: the .NET resolution decides (and names
/// a hint or startup project that is neither).
pub fn resolve_cargo(
    hint: Option<&str>,
    startup: Option<&Path>,
    dotnet_default: bool,
    cargo: Option<&CargoContext>,
) -> Option<CargoPackageInfo> {
    let ctx = cargo?;
    match hint {
        Some(h) => ctx.member(h).cloned(),
        None => startup
            .and_then(|s| ctx.member_at(s))
            .or_else(|| (!dotnet_default).then(|| ctx.default_member()).flatten())
            .cloned(),
    }
}

/// Whether a start without a hint runs a .NET project: the startup project is one of the solution's, or the solution
/// has an executable project (Visual Studio's default). Reads project files: off the UI thread.
pub fn dotnet_default(hint: Option<&str>, startup: Option<&Path>, projects: &[PathBuf]) -> bool {
    hint.is_none()
        && (startup.is_some_and(|s| {
            projects
                .iter()
                .any(|p| normalize_path(p) == normalize_path(s))
        }) || eludite_dap::launch::startup_project(projects).is_some())
}

/// The launch configuration of a Cargo package, computed on the launch thread.
#[derive(Debug, Clone)]
pub struct NativeLaunch {
    pub launch: cargo::CargoLaunch,
    pub config: LaunchConfig,
    pub init_commands: Vec<String>,
    /// `<sysroot>/lib/rustlib/src/rust`, when the `rust-src` component is installed.
    pub rust_src: Option<PathBuf>,
}

impl NativeLaunch {
    /// lldb-dap's `launch` arguments.
    pub fn arguments(&self) -> Value {
        self.launch.lldb_arguments(&self.init_commands)
    }
}

/// Compute the launch of `package`: build the test executable when the start asks for it (cargo's progress and
/// messages go to `console`), find the binary otherwise, and for a debugging session read the sysroot once for the
/// formatters' `initCommands` and `rust-src`. Runs processes: the launch thread only.
pub fn prepare(
    job: &NativeJob,
    package: &CargoPackageInfo,
    debug: bool,
    mut console: impl FnMut(&str),
) -> Result<NativeLaunch, String> {
    let ctx = job
        .cargo
        .as_ref()
        .ok_or("no Cargo workspace is open (File > Open > Workspace...)")?;
    let start = CargoStart {
        package: package.clone(),
        root: ctx.root.clone(),
        workspace_manifest: ctx.manifest.clone(),
        target_dir: ctx.target_dir.clone(),
        release: ctx.release,
        target: job.options.target.clone(),
        test: job.options.test,
        args: job.options.args.clone(),
    };
    let launch = if job.options.test {
        console(&format!(
            "Building the test executable of {}\u{2026}",
            package.name
        ));
        let artifacts = start.build_tests(&ctx.cargo, &mut console)?;
        start.test_launch(&artifacts)?
    } else {
        start.binary_launch(&[])?
    };
    let config = LaunchConfig::from_cargo(&launch);
    let mut init_commands = vec![cargo::STEP_AVOID.to_owned()];
    let mut rust_src = None;
    if debug {
        let sysroot = match &job.setup.sysroot {
            Some(s) => Ok(s.clone()),
            None => cargo::sysroot(&cargo::rustc_for(&ctx.cargo), &ctx.root),
        };
        match sysroot {
            Ok(sysroot) => {
                rust_src = cargo::rust_src(&sysroot);
                if job.setup.formatters {
                    let etc = cargo::formatters_dir(&sysroot);
                    let commands = cargo::rust_init_commands(&etc);
                    if commands.is_empty() {
                        console(&format!(
                            "The Rust formatters were not found in {}; values show as LLDB sees them.",
                            etc.display()
                        ));
                    }
                    init_commands.extend(commands);
                }
            }
            Err(e) => console(&format!(
                "{e}; values show as LLDB sees them and standard library frames as external code."
            )),
        }
    }
    Ok(NativeLaunch {
        launch,
        config,
        init_commands,
        rust_src,
    })
}

/// Reach the adapter: the tests' connector, or lldb-dap (or CodeLLDB) found by the search and started on stdio. Returns
/// the connection and `session.adapter`'s text (`lldb-dap 18.1.3 (stdio)`; the version read once here).
pub fn connect(
    job: &NativeJob,
    platform: Platform,
    connector: Option<&Connector>,
) -> Result<(Connection, String), String> {
    if let Some(connect) = connector {
        let c = connect().map_err(|e| format!("cannot reach the debug adapter: {e}"))?;
        let d = format!("lldb-dap ({})", c.description);
        return Ok((c, d));
    }
    let adapter = job.setup.lldb.find(platform)?;
    let version = adapter.version();
    let c = transport::connect(&adapter.transport())
        .map_err(|e| format!("cannot start {}: {e}", adapter.name()))?;
    Ok((c, adapter.describe(version.as_deref())))
}

/// The function breakpoints of a native session: `rust_panic` while the Rust panics row is on.
pub fn function_breakpoints(rust_panics: bool) -> Vec<FunctionBreakpoint> {
    if rust_panics {
        vec![FunctionBreakpoint {
            name: RUST_PANIC.to_owned(),
            ..Default::default()
        }]
    } else {
        Vec::new()
    }
}

/// One `setFunctionBreakpoints` list for a native session (DAP replaces the whole list on every request): the user's
/// function breakpoints (brief 0026) in their order, then `rust_panic` while the Rust panics row is on, unless the user
/// already has it.
pub fn with_rust_panics(
    mut user: Vec<FunctionBreakpoint>,
    rust_panics: bool,
) -> Vec<FunctionBreakpoint> {
    for f in function_breakpoints(rust_panics) {
        if !user.iter().any(|u| u.name == f.name) {
            user.push(f);
        }
    }
    user
}

/// What the shell takes from lldb-dap's capabilities: its hit conditions are not Visual Studio's (lldb-dap 18 reads
/// `hitCondition` as a number N and breaks on the Nth hit and every one after; `>=N` and `%N` do not parse, so it
/// breaks on every hit), so the shell counts hits itself, as it does for netcoredbg (brief 0018).
pub fn adapt_capabilities(caps: &mut Capabilities) {
    caps.supports_hit_conditional_breakpoints = false;
}

/// A line for the Debug source from the launch thread (cargo's test build), as the adapter's console output.
pub fn console_event(line: &str) -> ClientEvent {
    ClientEvent::Event(Event::Output(OutputEvent {
        category: Some("console".into()),
        output: format!("{line}\n"),
    }))
}

/// What lldb-dap 18 says, as the shell expects it (on the client's reader thread): a stop described `signal SIGSTOP`
/// (what `pause` causes) is reason `pause`; a standard library frame's `/rustc/<commit>/` path is mapped under
/// `rust_src`, or removed (the frame is then external code, `subtle`) when `rust-src` is not installed; `setVariable`'s
/// answer names the new value `result` where DAP says `value` (brief 0026's `set_variable` reads `value`).
pub fn adapt(event: ClientEvent, rust_src: Option<&Path>) -> ClientEvent {
    match event {
        ClientEvent::Response {
            request_seq,
            command,
            result: Ok(mut body),
        } if command == "setVariable" => {
            if body.get("value").is_none()
                && let Some(result) = body.get("result").cloned()
                && let Some(o) = body.as_object_mut()
            {
                o.insert("value".into(), result);
            }
            ClientEvent::Response {
                request_seq,
                command,
                result: Ok(body),
            }
        }
        ClientEvent::Event(Event::Stopped(mut s))
            if s.reason == "exception" && s.description.as_deref() == Some("signal SIGSTOP") =>
        {
            s.reason = "pause".into();
            ClientEvent::Event(Event::Stopped(s))
        }
        ClientEvent::Response {
            request_seq,
            command,
            result: Ok(mut body),
        } if command == "stackTrace" => {
            if let Some(frames) = body.get_mut("stackFrames").and_then(Value::as_array_mut) {
                for f in frames {
                    adapt_frame(f, rust_src);
                }
            }
            ClientEvent::Response {
                request_seq,
                command,
                result: Ok(body),
            }
        }
        other => other,
    }
}

fn adapt_frame(frame: &mut Value, rust_src: Option<&Path>) {
    let Some(path) = frame["source"]["path"].as_str().map(str::to_owned) else {
        return;
    };
    if !cargo::is_rustc_path(&path) {
        return;
    }
    match rust_src.and_then(|src| cargo::map_rustc_path(&path, src)) {
        Some(local) => {
            frame["source"]["path"] = Value::String(local.to_string_lossy().into_owned())
        }
        None => {
            if let Some(source) = frame["source"].as_object_mut() {
                source.remove("path");
            }
            frame["presentationHint"] = Value::String("subtle".into());
        }
    }
}

impl Shell {
    /// The open folder's Cargo workspace for the launch thread, with the toolbar's configuration and the cargo builds
    /// use.
    pub(super) fn cargo_context(&self) -> Option<CargoContext> {
        let ws = self.cargo_workspace()?;
        Some(CargoContext::from_workspace(
            ws,
            self.builds.configuration.eq_ignore_ascii_case("release"),
            PathBuf::from(&self.builds.cargo_program),
        ))
    }

    /// Where a folder with a Cargo workspace and no solution keeps what a solution keeps beside its breakpoints (the
    /// breakpoints, watches, exception settings and startup project): keyed by the folder's `Cargo.toml`.
    fn native_store_key(&self) -> Option<PathBuf> {
        let f = self.folder()?;
        f.solution
            .is_none()
            .then(|| f.cargo_manifest.clone())
            .flatten()
    }

    /// A folder opened: without a solution, its Cargo workspace's persisted debugger state is loaded (the solution's is
    /// loaded when the host opens it).
    pub(in crate::shell) fn debug_folder_opened(&mut self, cx: &mut Context<Self>) {
        if let Some(key) = self.native_store_key() {
            self.debug_solution_opened(&key, cx);
        }
    }

    /// Set as Startup Project on a member package of the open folder's Cargo workspace: kept as the package's
    /// `Cargo.toml` in the same per-solution store (`startup_project`), drawn bold. `None` when `project` names no
    /// member (the .NET path then answers).
    pub(in crate::shell) fn set_cargo_startup(
        &mut self,
        project: &str,
        cx: &mut Context<Self>,
    ) -> Option<Result<ProjectOutput, CommandError>> {
        let ctx = self.cargo_context()?;
        let member = ctx.member(project)?.clone();
        self.debug.model.startup_project = Some(member.manifest.to_string_lossy().into_owned());
        self.debug_persist(cx);
        self.refresh_startup(cx);
        super::super::documents::trace(format_args!(
            "startup project: {} ({})",
            member.name,
            member.manifest.display()
        ));
        let solution = self
            .solution
            .clone()
            .or_else(|| self.native_store_key())
            .unwrap_or_else(|| ctx.manifest.clone());
        Some(Ok(ProjectOutput::StartupProject(StartupProjectOutput {
            projects: Vec::new(),
            project: member.name,
            path: member.manifest.to_string_lossy().into_owned(),
            solution: solution.to_string_lossy().into_owned(),
        })))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use eludite_dap::types::StoppedEvent;
    use serde_json::json;

    fn ctx() -> CargoContext {
        let pkg = |name: &str, dir: &str, bins: &[&str]| CargoPackageInfo {
            name: name.into(),
            manifest: PathBuf::from(format!("/w/{dir}Cargo.toml")),
            bins: bins.iter().map(|b| (*b).to_owned()).collect(),
            has_lib: bins.is_empty(),
        };
        CargoContext {
            root: "/w".into(),
            manifest: "/w/Cargo.toml".into(),
            target_dir: "/w/target".into(),
            members: vec![
                pkg("core", "crates/core/", &[]),
                pkg("tool", "crates/tool/", &["tool"]),
                pkg("app", "crates/app/", &["app", "helper"]),
            ],
            release: false,
            cargo: "cargo".into(),
        }
    }

    #[test]
    fn the_package_comes_from_the_hint_the_startup_project_or_the_first_binary() {
        let c = ctx();
        let name = |p: Option<CargoPackageInfo>| p.map(|p| p.name);
        assert_eq!(
            name(resolve_cargo(Some("app"), None, true, Some(&c))),
            Some("app".into())
        );
        assert_eq!(
            name(resolve_cargo(
                Some("crates/tool/Cargo.toml"),
                None,
                true,
                Some(&c)
            )),
            Some("tool".into())
        );
        assert_eq!(
            name(resolve_cargo(Some("/w/crates/core"), None, false, Some(&c))),
            Some("core".into())
        );
        // A name that is no member is a .NET project's.
        assert_eq!(
            resolve_cargo(Some("App.csproj"), None, false, Some(&c)),
            None
        );
        // The startup project, when it is a member.
        let startup = Path::new("/w/crates/app/Cargo.toml");
        assert_eq!(
            name(resolve_cargo(None, Some(startup), true, Some(&c))),
            Some("app".into())
        );
        assert_eq!(
            resolve_cargo(
                None,
                Some(Path::new("/w/src/App/App.csproj")),
                true,
                Some(&c)
            ),
            None
        );
        // Nothing chosen: an executable .NET project first, else the first member with a binary.
        assert_eq!(resolve_cargo(None, None, true, Some(&c)), None);
        assert_eq!(
            name(resolve_cargo(None, None, false, Some(&c))),
            Some("tool".into())
        );
        assert_eq!(resolve_cargo(None, None, false, None), None);
    }

    #[test]
    fn lldb_dap_18_s_pause_and_standard_library_frames_are_adapted() {
        let pause = ClientEvent::Event(Event::Stopped(StoppedEvent {
            reason: "exception".into(),
            description: Some("signal SIGSTOP".into()),
            thread_id: Some(7),
            ..Default::default()
        }));
        match adapt(pause, None) {
            ClientEvent::Event(Event::Stopped(s)) => assert_eq!(s.reason, "pause"),
            other => panic!("{other:?}"),
        }
        let other = ClientEvent::Event(Event::Stopped(StoppedEvent {
            reason: "exception".into(),
            description: Some("signal SIGSEGV".into()),
            ..Default::default()
        }));
        assert_eq!(adapt(other.clone(), None), other);
        // setVariable's `result` is DAP's `value` (lldb-dap 18).
        let set = |body: Value| ClientEvent::Response {
            request_seq: 2,
            command: "setVariable".into(),
            result: Ok(body),
        };
        match adapt(set(json!({"result": "40", "variablesReference": 0})), None) {
            ClientEvent::Response { result: Ok(b), .. } => assert_eq!(b["value"], "40"),
            other => panic!("{other:?}"),
        }
        let dap = set(json!({"value": "41", "result": "x"}));
        assert_eq!(adapt(dap.clone(), None), dap);
        let stack = || ClientEvent::Response {
            request_seq: 3,
            command: "stackTrace".into(),
            result: Ok(json!({"stackFrames": [
                {"id": 1, "name": "app::main", "line": 4, "column": 5, "source": {"name": "main.rs", "path": "/w/src/main.rs"}},
                {"id": 2, "name": "std::rt::lang_start", "line": 206, "column": 18,
                 "source": {"name": "rt.rs", "path": "/rustc/48a229ceaefd4985c50990b14116b6d856af0985/library/std/src/rt.rs"}}
            ]})),
        };
        let frames = |e: ClientEvent| match e {
            ClientEvent::Response { result: Ok(b), .. } => b["stackFrames"].clone(),
            other => panic!("{other:?}"),
        };
        // Without rust-src: no path, external.
        let f = frames(adapt(stack(), None));
        assert_eq!(f[0]["source"]["path"], "/w/src/main.rs");
        assert_eq!(f[1]["source"].get("path"), None);
        assert_eq!(f[1]["presentationHint"], "subtle");
        // With it: the local copy.
        let f = frames(adapt(
            stack(),
            Some(Path::new("/sysroot/lib/rustlib/src/rust")),
        ));
        assert_eq!(
            Path::new(f[1]["source"]["path"].as_str().unwrap()),
            Path::new("/sysroot/lib/rustlib/src/rust/library/std/src/rt.rs")
        );
        assert_eq!(f[1].get("presentationHint"), None);
        assert_eq!(function_breakpoints(true)[0].name, "rust_panic");
        let mut caps = Capabilities {
            supports_hit_conditional_breakpoints: true,
            supports_log_points: true,
            ..Default::default()
        };
        adapt_capabilities(&mut caps);
        assert!(!caps.supports_hit_conditional_breakpoints && caps.supports_log_points);
        assert!(function_breakpoints(false).is_empty());
        // With the user's function breakpoints (brief 0026): one list, the user's first, `rust_panic` once.
        let user = |names: &[&str]| -> Vec<FunctionBreakpoint> {
            names
                .iter()
                .map(|n| FunctionBreakpoint {
                    name: (*n).to_owned(),
                    ..Default::default()
                })
                .collect()
        };
        let names = |l: Vec<FunctionBreakpoint>| l.into_iter().map(|f| f.name).collect::<Vec<_>>();
        assert_eq!(
            names(with_rust_panics(user(&["app::run"]), true)),
            ["app::run", "rust_panic"]
        );
        assert_eq!(
            names(with_rust_panics(user(&["app::run"]), false)),
            ["app::run"]
        );
        assert_eq!(
            names(with_rust_panics(user(&["rust_panic", "app::run"]), true)),
            ["rust_panic", "app::run"]
        );
        assert!(with_rust_panics(Vec::new(), false).is_empty());
    }
}
