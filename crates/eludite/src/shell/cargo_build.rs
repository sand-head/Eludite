//! The Cargo build path (brief 0019): `cargo build --message-format=json-diagnostic-rendered-ansi`, run by the shell
//! off the UI thread, reported in the `eludite/build/*` shapes the host uses for MSBuild, so the same Output window,
//! Error List, status bar and cancel serve both (host-rpc.md, "Generic language servers and Cargo").
//!
//! - **Start line first.** `Build started at ...` and the command line are sent before cargo is spawned, so the
//!   Output window shows a line at once.
//! - **Output.** Cargo's own stderr lines (`Compiling ...`, `Finished ...`) and each compiler message's `rendered`
//!   text, in arrival order, chunked like the host's (flushed every 16 ms or 16 KiB). The Output window removes the
//!   ANSI colors.
//! - **Progress.** `compiler-artifact` messages of workspace members count completed packages; errors and warnings
//!   are counted from `compiler-message`s. At most every 100 ms.
//! - **Diagnostics.** Each `compiler-message` with level `error` or `warning` becomes a build diagnostic at its
//!   primary span (file relative to the workspace root, 1-based line and column, end), with the rustc code
//!   (`E0308`) or the lint name (`unused_variables`) as code and the package's `Cargo.toml` as project. Notes, helps
//!   and failure notes are not rows. The same diagnostic reported twice is listed once.
//! - **Cancel** kills cargo's process group (rustc included): `kill -KILL -<pgid>` on Unix, `taskkill /T /F` on
//!   Windows; the finished notification reports `canceled`.
//! - **Rebuild** runs `cargo clean` first; **Clean** runs it alone. Release is `--release`; one package is `-p`.

use std::collections::{BTreeMap, HashSet};
use std::ffi::OsString;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use eludite_commands::build::BuildKind;
use eludite_lsp::host::{
    BuildDiagnostic, BuildDiagnosticSeverity, BuildFinished, BuildOutput, BuildProgress,
    BuildProjectResult, BuildResult, BuildStartResult, BuildSummary, BuildSystem, BuildTarget,
    Toolchain, ToolchainKind,
};
use futures::channel::mpsc::UnboundedSender;
use serde_json::Value;

use super::session::SessionEvent;

/// Cargo build ids start here, apart from the host's (which count from 1 in each host process).
pub const CARGO_BUILD_ID_BASE: u64 = 1 << 32;

/// Output chunks are flushed when they reach this size or this age (as the host's).
const CHUNK_BYTES: usize = 16 * 1024;
const CHUNK_AGE: Duration = Duration::from_millis(16);
const PROGRESS_EVERY: Duration = Duration::from_millis(100);

/// A member package of the workspace being built.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Member {
    pub name: String,
    pub manifest: PathBuf,
}

/// One Cargo build.
#[derive(Debug, Clone)]
pub struct CargoBuildSpec {
    pub ticket: u64,
    pub id: u64,
    /// The solution generation when it started (the shell's stale check on finished builds).
    pub generation: u64,
    pub kind: BuildKind,
    /// The workspace's `Cargo.toml`.
    pub manifest: PathBuf,
    pub root: PathBuf,
    /// One package (`-p`), or the workspace.
    pub package: Option<String>,
    pub release: bool,
    pub members: Vec<Member>,
    /// `cargo` (tests may point it elsewhere).
    pub program: OsString,
    /// Build the test executables instead (`cargo test --no-run`, the Test Explorer's build; brief 0035).
    pub tests: bool,
}

impl CargoBuildSpec {
    fn configuration(&self) -> &'static str {
        if self.release { "Release" } else { "Debug" }
    }

    /// The arguments of one cargo invocation (`build` or `clean`).
    pub fn args(&self, subcommand: &str) -> Vec<String> {
        let mut args = vec![subcommand.to_owned()];
        if subcommand == "build" && self.tests {
            args = vec!["test".into(), "--no-run".into()];
        }
        if subcommand == "build" {
            args.push("--message-format=json-diagnostic-rendered-ansi".into());
        }
        args.push("--manifest-path".into());
        args.push(self.manifest.to_string_lossy().into_owned());
        if let Some(p) = &self.package {
            args.push("-p".into());
            args.push(p.clone());
        }
        if self.release {
            args.push("--release".into());
        }
        args
    }

    fn command_line(&self) -> String {
        let program = self.program.to_string_lossy();
        let line = |sub: &str| format!("{program} {}", self.args(sub).join(" "));
        match self.kind {
            BuildKind::Build => line("build"),
            BuildKind::Rebuild => format!("{} && {}", line("clean"), line("build")),
            BuildKind::Clean => line("clean"),
        }
    }
}

/// A Cargo build in flight.
#[derive(Debug, Clone)]
pub struct CargoRun {
    child: Arc<Mutex<Option<u32>>>,
    canceled: Arc<AtomicBool>,
}

impl CargoRun {
    /// Kill cargo and everything it started; the finished notification reports `canceled`.
    pub fn cancel(&self) {
        self.canceled.store(true, Ordering::SeqCst);
        if let Some(pid) = *self.child.lock().unwrap_or_else(|e| e.into_inner()) {
            kill_tree(pid);
        }
    }
}

#[cfg(unix)]
pub(super) fn kill_tree(pid: u32) {
    // cargo runs in its own process group (`process_group(0)`), so the group id is its pid.
    let _ = Command::new("kill")
        .args(["-KILL", "--", &format!("-{pid}")])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

#[cfg(windows)]
pub(super) fn kill_tree(pid: u32) {
    let _ = Command::new("taskkill")
        .args(["/T", "/F", "/PID", &pid.to_string()])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

/// What one line of cargo's JSON output says.
#[derive(Debug, Clone, PartialEq)]
pub enum CargoLine {
    /// A compiler message: its rendered text, and the diagnostic when it is an error or warning row.
    Message {
        rendered: Option<String>,
        diagnostic: Option<BuildDiagnostic>,
    },
    /// A unit finished building (or was fresh), of this package.
    Artifact {
        manifest: Option<PathBuf>,
    },
    /// The end of the build.
    Finished {
        success: bool,
    },
    /// Something else on stdout (not JSON): shown as is.
    Text(String),
    Other,
}

/// Parse one line of `cargo build --message-format=json-diagnostic-rendered-ansi`. Relative file names are under
/// `root` (cargo runs rustc in the workspace root).
pub fn parse_line(line: &str, root: &Path) -> CargoLine {
    let Ok(v) = serde_json::from_str::<Value>(line) else {
        return CargoLine::Text(line.to_owned());
    };
    let manifest = v
        .get("manifest_path")
        .and_then(Value::as_str)
        .map(PathBuf::from);
    match v.get("reason").and_then(Value::as_str) {
        Some("compiler-message") => {
            let m = &v["message"];
            CargoLine::Message {
                rendered: m["rendered"].as_str().map(str::to_owned),
                diagnostic: diagnostic(m, root, manifest.as_deref()),
            }
        }
        Some("compiler-artifact") => CargoLine::Artifact { manifest },
        Some("build-finished") => CargoLine::Finished {
            success: v["success"].as_bool().unwrap_or(false),
        },
        _ => CargoLine::Other,
    }
}

/// A rustc diagnostic (`message` of a `compiler-message`) as a build diagnostic, when it is an error or warning.
pub fn diagnostic(m: &Value, root: &Path, manifest: Option<&Path>) -> Option<BuildDiagnostic> {
    let severity = match m["level"].as_str()? {
        "error" | "error: internal compiler error" => BuildDiagnosticSeverity::Error,
        "warning" => BuildDiagnosticSeverity::Warning,
        // note, help, failure-note: part of another message, or the "rustc --explain" hint.
        _ => return None,
    };
    let primary = m["spans"]
        .as_array()
        .and_then(|s| s.iter().find(|s| s["is_primary"].as_bool() == Some(true)));
    let file = primary.and_then(|s| s["file_name"].as_str()).map(|f| {
        let p = Path::new(f);
        let abs = if p.is_absolute() {
            p.to_path_buf()
        } else {
            root.join(p)
        };
        super::documents::normalize_path(&abs)
            .to_string_lossy()
            .into_owned()
    });
    let num = |k: &str| {
        primary
            .and_then(|s| s[k].as_u64())
            .map(|n| n.min(u32::MAX as u64) as u32)
    };
    Some(BuildDiagnostic {
        severity,
        code: m["code"]["code"].as_str().unwrap_or_default().to_owned(),
        message: m["message"].as_str().unwrap_or_default().to_owned(),
        file,
        line: num("line_start"),
        column: num("column_start"),
        end_line: num("line_end"),
        end_column: num("column_end"),
        project: manifest.map(|p| p.to_string_lossy().into_owned()),
    })
}

/// A diagnostic's identity: file, line, column, code, message.
type DiagnosticKey = (Option<String>, Option<u32>, Option<u32>, String, String);

/// What the runner collects while cargo runs.
#[derive(Debug, Default)]
pub struct Collected {
    pub diagnostics: Vec<BuildDiagnostic>,
    seen: HashSet<DiagnosticKey>,
    pub errors: u32,
    pub warnings: u32,
    /// Members whose units finished, by manifest.
    pub built: HashSet<PathBuf>,
    pub success: Option<bool>,
}

impl Collected {
    /// Take one parsed line; returns the text to show in the Output window.
    pub fn take(&mut self, line: CargoLine) -> Option<String> {
        match line {
            CargoLine::Message {
                rendered,
                diagnostic,
            } => {
                if let Some(d) = diagnostic {
                    let key = (
                        d.file.clone(),
                        d.line,
                        d.column,
                        d.code.clone(),
                        d.message.clone(),
                    );
                    if self.seen.insert(key) {
                        match d.severity {
                            BuildDiagnosticSeverity::Error => self.errors += 1,
                            BuildDiagnosticSeverity::Warning => self.warnings += 1,
                            BuildDiagnosticSeverity::Message => {}
                        }
                        self.diagnostics.push(d);
                    }
                }
                rendered
            }
            CargoLine::Artifact { manifest } => {
                if let Some(m) = manifest {
                    self.built.insert(m);
                }
                None
            }
            CargoLine::Finished { success } => {
                self.success = Some(success);
                None
            }
            CargoLine::Text(t) => Some(format!("{t}\n")),
            CargoLine::Other => None,
        }
    }

    /// The per-package results: every member that built or reported, failed when it has an error.
    pub fn projects(&self, members: &[Member], canceled: bool) -> Vec<BuildProjectResult> {
        let mut by: BTreeMap<String, BuildProjectResult> = BTreeMap::new();
        for m in members {
            let key = m.manifest.to_string_lossy().into_owned();
            let errors = self.count(&key, BuildDiagnosticSeverity::Error);
            let warnings = self.count(&key, BuildDiagnosticSeverity::Warning);
            if !self.built.contains(&m.manifest) && errors == 0 && warnings == 0 {
                continue;
            }
            let result = if errors > 0 {
                BuildResult::Failed
            } else if canceled && !self.built.contains(&m.manifest) {
                BuildResult::Canceled
            } else {
                BuildResult::Succeeded
            };
            by.insert(
                m.name.clone(),
                BuildProjectResult {
                    name: m.name.clone(),
                    path: key,
                    result,
                    elapsed_ms: None,
                    errors,
                    warnings,
                },
            );
        }
        by.into_values().collect()
    }

    fn count(&self, manifest: &str, severity: BuildDiagnosticSeverity) -> u32 {
        self.diagnostics
            .iter()
            .filter(|d| d.severity == severity && d.project.as_deref() == Some(manifest))
            .count() as u32
    }
}

enum Input {
    Out(String),
    Err(String),
}

/// Start `spec` on a thread; its events (started, output, progress, finished) go to `events`.
pub fn start(spec: CargoBuildSpec, events: UnboundedSender<SessionEvent>) -> CargoRun {
    let run = CargoRun {
        child: Arc::default(),
        canceled: Arc::default(),
    };
    let handle = run.clone();
    let spawned = thread::Builder::new()
        .name("eludite-cargo-build".into())
        .spawn(move || run_build(spec, events, handle));
    if let Err(e) = spawned {
        eprintln!("eludite: cannot start the cargo build thread: {e}");
    }
    run
}

struct Out<'a> {
    events: &'a UnboundedSender<SessionEvent>,
    id: u64,
    seq: u64,
    pending: String,
    since: Option<Instant>,
}

impl Out<'_> {
    fn push(&mut self, text: &str) {
        if self.pending.is_empty() {
            self.since = Some(Instant::now());
        }
        self.pending.push_str(text);
        if !text.ends_with('\n') {
            self.pending.push('\n');
        }
        if self.pending.len() >= CHUNK_BYTES || self.since.is_some_and(|s| s.elapsed() >= CHUNK_AGE)
        {
            self.flush();
        }
    }

    fn flush(&mut self) {
        if self.pending.is_empty() {
            return;
        }
        let text = std::mem::take(&mut self.pending);
        let _ = self
            .events
            .unbounded_send(SessionEvent::BuildOutput(BuildOutput {
                build_id: self.id,
                seq: self.seq,
                text,
            }));
        self.seq += 1;
        self.since = None;
    }
}

fn run_build(spec: CargoBuildSpec, events: UnboundedSender<SessionEvent>, run: CargoRun) {
    let started = Instant::now();
    let target = match spec.kind {
        BuildKind::Build => BuildTarget::Build,
        BuildKind::Rebuild => BuildTarget::Rebuild,
        BuildKind::Clean => BuildTarget::Clean,
    };
    let command_line = spec.command_line();
    let _ = events.unbounded_send(SessionEvent::BuildStarted {
        ticket: spec.ticket,
        result: BuildStartResult {
            build_id: spec.id,
            generation: spec.generation,
            system: Some(BuildSystem::Cargo),
            path: spec.manifest.to_string_lossy().into_owned(),
            target,
            configuration: spec.configuration().into(),
            platform: None,
            toolchain: Toolchain {
                kind: ToolchainKind::Cargo,
                path: Some(spec.program.to_string_lossy().into_owned()),
                source: None,
            },
            binlog: None,
            command_line: command_line.clone(),
        },
    });
    let mut out = Out {
        events: &events,
        id: spec.id,
        seq: 0,
        pending: String::new(),
        since: None,
    };
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let (h, m, s) = ((now / 3600) % 24, (now / 60) % 60, now % 60);
    out.push(&format!(
        "Build started at {h:02}:{m:02}:{s:02} UTC...\n> {command_line}"
    ));
    out.flush();

    let mut collected = Collected::default();
    let mut exit: Option<i32> = None;
    let mut failure: Option<String> = None;
    let steps: &[&str] = match spec.kind {
        BuildKind::Build => &["build"],
        BuildKind::Rebuild => &["clean", "build"],
        BuildKind::Clean => &["clean"],
    };
    let mut last_progress = Instant::now() - PROGRESS_EVERY;
    for step in steps {
        if run.canceled.load(Ordering::SeqCst) {
            break;
        }
        let mut cmd = Command::new(&spec.program);
        cmd.args(spec.args(step))
            .current_dir(&spec.root)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt as _;
            cmd.process_group(0);
        }
        let mut child: Child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => {
                failure = Some(format!(
                    "{} could not start: {e}",
                    spec.program.to_string_lossy()
                ));
                out.push(failure.as_deref().unwrap_or_default());
                break;
            }
        };
        *run.child.lock().unwrap_or_else(|e| e.into_inner()) = Some(child.id());
        if run.canceled.load(Ordering::SeqCst) {
            kill_tree(child.id());
        }
        let (tx, rx) = mpsc::channel::<Input>();
        let readers = [
            child.stdout.take().map(|s| {
                let tx = tx.clone();
                thread::spawn(move || {
                    for line in BufReader::new(s).lines().map_while(Result::ok) {
                        if tx.send(Input::Out(line)).is_err() {
                            break;
                        }
                    }
                })
            }),
            child.stderr.take().map(|s| {
                let tx = tx.clone();
                thread::spawn(move || {
                    for line in BufReader::new(s).lines().map_while(Result::ok) {
                        if tx.send(Input::Err(line)).is_err() {
                            break;
                        }
                    }
                })
            }),
        ];
        drop(tx);
        loop {
            match rx.recv_timeout(CHUNK_AGE) {
                Ok(Input::Out(line)) => {
                    if let Some(text) = collected.take(parse_line(&line, &spec.root)) {
                        out.push(&text);
                    }
                }
                Ok(Input::Err(line)) => out.push(&line),
                Err(mpsc::RecvTimeoutError::Timeout) => out.flush(),
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }
            if last_progress.elapsed() >= PROGRESS_EVERY {
                last_progress = Instant::now();
                send_progress(&events, &spec, &collected, started);
            }
        }
        for r in readers.into_iter().flatten() {
            let _ = r.join();
        }
        let status = child.wait();
        *run.child.lock().unwrap_or_else(|e| e.into_inner()) = None;
        exit = status.ok().and_then(|s| s.code());
        if exit != Some(0) {
            break;
        }
    }
    let canceled = run.canceled.load(Ordering::SeqCst);
    let result = if canceled {
        BuildResult::Canceled
    } else if exit == Some(0) && failure.is_none() {
        BuildResult::Succeeded
    } else {
        BuildResult::Failed
    };
    let projects = collected.projects(&spec.members, canceled);
    let failed = projects
        .iter()
        .filter(|p| p.result == BuildResult::Failed)
        .count() as u32;
    let succeeded = projects.len() as u32 - failed;
    let elapsed_ms = started.elapsed().as_secs_f64() * 1e3;
    if canceled {
        out.push("Build canceled.");
    }
    out.push(&format!(
        "========== {}: {succeeded} succeeded, {failed} failed ==========\n========== {} {} and took {:.3} seconds ==========",
        match spec.kind {
            BuildKind::Clean => "Clean",
            BuildKind::Rebuild => "Rebuild All",
            BuildKind::Build => "Build",
        },
        match spec.kind {
            BuildKind::Clean => "Clean",
            _ => "Build",
        },
        match result {
            BuildResult::Succeeded => "completed",
            BuildResult::Failed => "failed",
            BuildResult::Canceled => "canceled",
        },
        elapsed_ms / 1e3
    ));
    out.flush();
    let _ = events.unbounded_send(SessionEvent::BuildFinished {
        finished: Box::new(BuildFinished {
            build_id: spec.id,
            generation: spec.generation,
            target,
            path: spec.manifest.to_string_lossy().into_owned(),
            result,
            exit_code: exit,
            elapsed_ms,
            summary: BuildSummary {
                projects_succeeded: succeeded,
                projects_failed: failed,
                errors: collected.errors,
                warnings: collected.warnings,
            },
            projects,
            diagnostics: collected.diagnostics,
            diagnostics_truncated: false,
            binlog: None,
            message: failure,
        }),
        received: Instant::now(),
    });
}

fn send_progress(
    events: &UnboundedSender<SessionEvent>,
    spec: &CargoBuildSpec,
    collected: &Collected,
    started: Instant,
) {
    let total = match &spec.package {
        Some(_) => 1,
        None => spec.members.len() as u32,
    };
    let completed = spec
        .members
        .iter()
        .filter(|m| collected.built.contains(&m.manifest))
        .count() as u32;
    let _ = events.unbounded_send(SessionEvent::BuildProgress(BuildProgress {
        build_id: spec.id,
        elapsed_ms: started.elapsed().as_secs_f64() * 1e3,
        projects_total: total,
        projects_completed: completed.min(total.max(completed)),
        errors: collected.errors,
        warnings: collected.warnings,
        current_project: None,
    }));
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `cargo build --message-format=json-diagnostic-rendered-ansi` of a two-member workspace with a warning in
    /// `util` and a type error in `app`, recorded at brief 0019 (paths under `/w/ws`).
    const RECORDED: &str = include_str!("testdata/cargo-build.jsonl");

    fn members() -> Vec<Member> {
        vec![
            Member {
                name: "app".into(),
                manifest: "/w/ws/app/Cargo.toml".into(),
            },
            Member {
                name: "util".into(),
                manifest: "/w/ws/util/Cargo.toml".into(),
            },
        ]
    }

    #[test]
    fn recorded_json_becomes_rows_output_and_project_results() {
        let root = Path::new("/w/ws");
        let mut c = Collected::default();
        let mut shown = String::new();
        for line in RECORDED.lines() {
            if let Some(t) = c.take(parse_line(line, root)) {
                shown.push_str(&t);
            }
        }
        assert_eq!((c.errors, c.warnings), (1, 1));
        assert_eq!(c.success, Some(false));
        let warning = &c.diagnostics[0];
        assert_eq!(warning.severity, BuildDiagnosticSeverity::Warning);
        assert_eq!(warning.code, "unused_variables");
        assert_eq!(warning.message, "unused variable: `unused`");
        assert_eq!(
            warning.file.as_deref(),
            Some(
                super::super::documents::normalize_path(Path::new("/w/ws/util/src/lib.rs"))
                    .to_string_lossy()
                    .as_ref()
            )
        );
        assert_eq!((warning.line, warning.column), (Some(2), Some(9)));
        let error = &c.diagnostics[1];
        assert_eq!(error.severity, BuildDiagnosticSeverity::Error);
        assert_eq!(error.code, "E0308");
        assert_eq!(error.message, "mismatched types");
        assert_eq!(
            (error.line, error.column, error.end_line, error.end_column),
            (Some(2), Some(30), Some(2), Some(33))
        );
        assert_eq!(error.project.as_deref(), Some("/w/ws/app/Cargo.toml"));
        // The rendered text is shown; the failure note ("rustc --explain") too, but it is no row.
        assert!(shown.contains("mismatched types"));
        assert!(shown.contains("rustc --explain E0308"));
        assert_eq!(c.diagnostics.len(), 2);
        let projects = c.projects(&members(), false);
        let by_name: Vec<(&str, BuildResult, u32, u32)> = projects
            .iter()
            .map(|p| (p.name.as_str(), p.result, p.errors, p.warnings))
            .collect();
        assert_eq!(
            by_name,
            [
                ("app", BuildResult::Failed, 1, 0),
                ("util", BuildResult::Succeeded, 0, 1)
            ]
        );
    }

    #[test]
    fn duplicates_are_listed_once_and_other_levels_are_not_rows() {
        let line = RECORDED
            .lines()
            .find(|l| l.contains("mismatched types"))
            .unwrap();
        let mut c = Collected::default();
        c.take(parse_line(line, Path::new("/w/ws")));
        c.take(parse_line(line, Path::new("/w/ws")));
        assert_eq!((c.errors, c.diagnostics.len()), (1, 1));
        let note = serde_json::json!({"level": "note", "message": "x", "spans": []});
        assert!(diagnostic(&note, Path::new("/"), None).is_none());
        let linker = serde_json::json!({"level": "error", "message": "linking with `cc` failed", "spans": [],
                                        "code": null});
        let d = diagnostic(&linker, Path::new("/"), None).unwrap();
        assert_eq!((d.file, d.line, d.code.as_str()), (None, None, ""));
        assert_eq!(
            parse_line("   Compiling app", Path::new("/")),
            CargoLine::Text("   Compiling app".into())
        );
    }

    #[test]
    fn command_lines() {
        let spec = CargoBuildSpec {
            ticket: 1,
            id: CARGO_BUILD_ID_BASE,
            generation: 0,
            kind: BuildKind::Rebuild,
            manifest: "/w/ws/Cargo.toml".into(),
            root: "/w/ws".into(),
            package: Some("app".into()),
            release: true,
            members: members(),
            program: "cargo".into(),
            tests: false,
        };
        assert_eq!(
            spec.args("build"),
            [
                "build",
                "--message-format=json-diagnostic-rendered-ansi",
                "--manifest-path",
                "/w/ws/Cargo.toml",
                "-p",
                "app",
                "--release"
            ]
        );
        assert_eq!(
            spec.command_line(),
            "cargo clean --manifest-path /w/ws/Cargo.toml -p app --release && cargo build \
             --message-format=json-diagnostic-rendered-ansi --manifest-path /w/ws/Cargo.toml -p app --release"
        );
        assert_eq!(spec.configuration(), "Release");
        // The Test Explorer's build (brief 0035): the test executables.
        let tests = CargoBuildSpec {
            tests: true,
            kind: BuildKind::Build,
            ..spec
        };
        assert_eq!(
            tests.args("build")[..3],
            [
                "test",
                "--no-run",
                "--message-format=json-diagnostic-rendered-ansi"
            ]
        );
    }
}
