//! Rust tests in the Test Explorer (brief 0035): `cargo test` run by the shell, as brief 0019 runs `cargo build`,
//! never through `eludite-host` (host-rpc.md, "Tests", Cargo).
//!
//! - **Building** is the Cargo build path's: `cargo test --no-run` (`CargoBuildSpec::tests`), so its output and errors
//!   are the Build pane's and the Error List's.
//! - **Listing.** One `cargo test -p <package> <--lib | --bin N | --test N> -- --list --format terse` per test target
//!   (libraries, binaries and integration tests; doc tests, examples and benches are not listed), each line
//!   `path::to::test: test`. A test's source and line are found by looking for `fn <name>` after a `#[test]` in the
//!   target's source folder ([`find_test_fn`]): libtest does not report locations.
//! - **Running.** One `cargo test` per package and target with `-- --exact <names> --nocapture --test-threads=1`
//!   (`--test-threads` and `--nocapture` dropped when the setting test.parallel is on), with `RUST_BACKTRACE=1`. The
//!   [`Libtest`] parser reads libtest's human output: `test <name> ... ok|FAILED|ignored[, reason]`, a test's own
//!   output between its `... ` and its outcome (one test at a time, `--nocapture`), the panics on stderr
//!   (`thread '<name>' (<tid>) panicked at <file>:<line>:<col>:`, its message and backtrace), and the `---- <name>
//!   stdout ----` sections of a captured (parallel) run. Durations are measured between a test's start and outcome
//!   lines (libtest prints none on stable).
//! - **Cancel** kills cargo's process group, as the build's cancel does.

use std::collections::HashMap;
use std::ffi::OsString;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::Instant;

use eludite_commands::test::Outcome;
use eludite_workspace::cargo::{CargoPackage, TargetKind};
use futures::channel::mpsc::UnboundedSender;

/// A target of a package whose tests `cargo test` lists and runs.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TestTarget {
    pub kind: TargetKind,
    pub name: String,
    /// The target's root source file.
    pub src_path: PathBuf,
}

impl TestTarget {
    /// `lib`, `bin:<name>` or `test:<name>`: part of a test's id.
    pub fn key(&self) -> String {
        match self.kind {
            TargetKind::Lib | TargetKind::ProcMacro => "lib".into(),
            k => format!("{}:{}", k.as_str(), self.name),
        }
    }

    /// The `cargo test` arguments that select it.
    pub fn flags(&self) -> Vec<String> {
        match self.kind {
            TargetKind::Lib | TargetKind::ProcMacro => vec!["--lib".into()],
            TargetKind::Bin => vec!["--bin".into(), self.name.clone()],
            _ => vec!["--test".into(), self.name.clone()],
        }
    }

    /// The prefix of its tests' full names: none for the library, the target's name for a binary or an integration
    /// test (`integration::adds_from_outside`).
    pub fn prefix(&self) -> Option<&str> {
        match self.kind {
            TargetKind::Lib | TargetKind::ProcMacro => None,
            _ => Some(&self.name),
        }
    }
}

/// The targets of `package` with tests: its library, binaries and integration tests, in that order.
pub fn test_targets(package: &CargoPackage) -> Vec<TestTarget> {
    let mut out: Vec<TestTarget> = package
        .targets
        .iter()
        .filter(|t| {
            matches!(
                t.kind,
                TargetKind::Lib | TargetKind::ProcMacro | TargetKind::Bin | TargetKind::Test
            )
        })
        .map(|t| TestTarget {
            kind: t.kind,
            name: t.name.clone(),
            src_path: t.src_path.clone(),
        })
        .collect();
    let rank = |k: TargetKind| match k {
        TargetKind::Lib | TargetKind::ProcMacro => 0,
        TargetKind::Bin => 1,
        _ => 2,
    };
    out.sort_by_key(|t| rank(t.kind));
    out
}

/// `cargo test` arguments that list `target`'s tests.
pub fn list_args(workspace_manifest: &Path, package: &str, target: &TestTarget) -> Vec<String> {
    let mut args = vec![
        "test".to_owned(),
        "--manifest-path".into(),
        workspace_manifest.to_string_lossy().into_owned(),
        "-p".into(),
        package.into(),
    ];
    args.extend(target.flags());
    args.extend([
        "--".into(),
        "--list".into(),
        "--format".into(),
        "terse".into(),
    ]);
    args
}

/// `cargo test` arguments that run `names` of `target` (all of them when empty).
pub fn run_args(
    workspace_manifest: &Path,
    package: &str,
    target: &TestTarget,
    names: &[String],
    parallel: bool,
    release: bool,
) -> Vec<String> {
    let mut args = vec![
        "test".to_owned(),
        "--manifest-path".into(),
        workspace_manifest.to_string_lossy().into_owned(),
        "-p".into(),
        package.into(),
    ];
    args.extend(target.flags());
    if release {
        args.push("--release".into());
    }
    args.push("--".into());
    if !names.is_empty() {
        args.push("--exact".into());
        args.extend(names.iter().cloned());
    }
    if !parallel {
        args.extend(["--nocapture".into(), "--test-threads=1".into()]);
    }
    args
}

/// The test names of `cargo test -- --list --format terse` (`path::name: test`; benchmarks are left out).
pub fn parse_list(stdout: &str) -> Vec<String> {
    stdout
        .lines()
        .filter_map(|l| l.strip_suffix(": test"))
        .map(|n| n.trim().to_owned())
        .filter(|n| !n.is_empty())
        .collect()
}

/// Where `fn <last segment of name>` is declared after a `#[test]` attribute, under the folder of the target's root
/// source file (at most 500 `.rs` files). A file named after one of the name's modules is preferred.
pub fn find_test_fn(src_path: &Path, name: &str) -> Option<(PathBuf, u32)> {
    let function = name.rsplit("::").next()?;
    let dir = src_path.parent()?;
    let mut files = Vec::new();
    collect_rs(dir, &mut files, 500);
    // The root file first, then files named after a module of the path, then the rest.
    let modules: Vec<&str> = name.split("::").collect();
    files.sort_by_key(|f| {
        let stem = f.file_stem().map(|s| s.to_string_lossy().into_owned());
        if f == src_path {
            0
        } else if stem.is_some_and(|s| modules.contains(&s.as_str())) {
            1
        } else {
            2
        }
    });
    let needle = format!("fn {function}(");
    for file in files {
        let Ok(text) = std::fs::read_to_string(&file) else {
            continue;
        };
        let lines: Vec<&str> = text.lines().collect();
        for (i, line) in lines.iter().enumerate() {
            if !line.contains(&needle) {
                continue;
            }
            let attributed = lines[i.saturating_sub(4)..i]
                .iter()
                .any(|l| l.trim_start().starts_with("#[test"));
            if attributed {
                return Some((file, i as u32 + 1));
            }
        }
    }
    None
}

fn collect_rs(dir: &Path, out: &mut Vec<PathBuf>, max: usize) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<_> = entries.flatten().map(|e| e.path()).collect();
    entries.sort();
    for p in entries {
        if out.len() >= max {
            return;
        }
        if p.is_dir() {
            if p.file_name().is_some_and(|n| n != "target") {
                collect_rs(&p, out, max);
            }
        } else if p.extension().is_some_and(|e| e == "rs") {
            out.push(p);
        }
    }
}

/// A test's state as libtest reported it.
#[derive(Debug, Clone, PartialEq)]
pub struct RustResult {
    /// The name libtest printed (`tests::adds`).
    pub name: String,
    pub outcome: Outcome,
    pub duration_ms: Option<f64>,
    /// The panic message, or the ignore reason.
    pub message: Option<String>,
    /// The panic's backtrace (with `RUST_BACKTRACE=1`), else its location.
    pub stack_trace: Option<String>,
    /// What the test printed.
    pub output: Option<String>,
    /// Where it panicked: the file as printed (relative to the workspace root) and the line.
    pub location: Option<(String, u32)>,
}

impl RustResult {
    fn new(name: &str, outcome: Outcome) -> Self {
        Self {
            name: name.to_owned(),
            outcome,
            duration_ms: None,
            message: None,
            stack_trace: None,
            output: None,
            location: None,
        }
    }
}

/// A panic as libtest's harness printed it.
#[derive(Debug, Clone, Default, PartialEq)]
struct Panic {
    location: Option<(String, u32)>,
    message: Vec<String>,
    backtrace: Vec<String>,
    in_backtrace: bool,
}

impl Panic {
    fn apply(&self, r: &mut RustResult) {
        if !self.message.is_empty() {
            r.message = Some(self.message.join("\n").trim_end().to_owned());
        }
        r.location = self.location.clone();
        let mut stack = self.backtrace.join("\n");
        if stack.is_empty()
            && let Some((file, line)) = &self.location
        {
            stack = format!("at {file}:{line}");
        }
        if !stack.is_empty() {
            r.stack_trace = Some(stack);
        }
    }
}

/// `thread 'tests::adds' (6228) panicked at src/lib.rs:25:9:` (the thread id is Rust 1.91's and later's) into the
/// name and the location.
fn panic_header(line: &str) -> Option<(String, Option<(String, u32)>)> {
    let rest = line.strip_prefix("thread '")?;
    let (name, rest) = rest.split_once('\'')?;
    let at = rest.find(" panicked at ")?;
    let place = rest[at + " panicked at ".len()..].trim_end_matches(':');
    let mut parts = place.rsplitn(3, ':');
    let _column = parts.next();
    let line_no = parts.next().and_then(|l| l.parse().ok());
    let file = parts.next();
    let location = match (file, line_no) {
        (Some(f), Some(l)) => Some((f.to_owned(), l)),
        _ => None,
    };
    Some((name.to_owned(), location))
}

/// libtest's human output, line by line, into results (see the module docs).
#[derive(Debug, Default)]
pub struct Libtest {
    /// The test whose output is being printed (`--nocapture`), with when it started and its output so far.
    current: Option<(String, Instant, String)>,
    /// When each test started (its `test <name> ... ` line).
    started: HashMap<String, Instant>,
    /// Panics by test name.
    panics: HashMap<String, Panic>,
    /// The panic being read from stderr.
    stderr_panic: Option<String>,
    /// A captured run's `---- <name> stdout ----` section being read.
    section: Option<(String, Vec<String>)>,
    /// Results already reported as failed, by name (the panic may come after them).
    failed: HashMap<String, RustResult>,
    in_failures: bool,
}

impl Libtest {
    /// Take one stdout line; returns the results it completes (and `running` for a test that started printing).
    pub fn stdout(&mut self, line: &str, now: Instant) -> Vec<RustResult> {
        let mut out = Vec::new();
        if let Some((name, lines)) = self.section.as_mut() {
            if line.starts_with("---- ") || line == "failures:" {
                let (name, lines) = (name.clone(), std::mem::take(lines));
                self.section = None;
                self.read_section(&name, &lines);
            } else {
                lines.push(line.to_owned());
                return out;
            }
        }
        if let Some(name) = line
            .strip_prefix("---- ")
            .and_then(|l| l.strip_suffix(" stdout ----"))
        {
            self.section = Some((name.to_owned(), Vec::new()));
            return out;
        }
        if line == "failures:" {
            self.in_failures = true;
            return out;
        }
        if let Some(rest) = line.strip_prefix("test ")
            && let Some((name, outcome)) = rest.split_once(" ... ")
            && !self.in_failures
        {
            let name = name.trim().to_owned();
            self.started.insert(name.clone(), now);
            match Self::outcome(outcome) {
                Some((o, reason)) => out.push(self.finish(&name, o, reason, None, now)),
                None => {
                    // `--nocapture`: the test prints before its outcome.
                    out.push(RustResult::new(&name, Outcome::Running));
                    self.current = Some((name, now, format!("{outcome}\n")));
                }
            }
            return out;
        }
        if let Some((name, _, mut output)) = self.current.take() {
            match Self::outcome(line) {
                Some((o, reason)) => {
                    out.push(self.finish(&name, o, reason, Some(output), now));
                }
                None => {
                    output.push_str(line);
                    output.push('\n');
                    let started = self.started.get(&name).copied().unwrap_or(now);
                    self.current = Some((name, started, output));
                }
            }
        }
        out
    }

    /// Take one stderr line (panics of a `--nocapture` run); returns failed results it completes.
    pub fn stderr(&mut self, line: &str) -> Vec<RustResult> {
        if let Some((name, location)) = panic_header(line) {
            self.end_stderr_panic();
            self.panics.insert(
                name.clone(),
                Panic {
                    location,
                    ..Panic::default()
                },
            );
            self.stderr_panic = Some(name);
            return Vec::new();
        }
        let Some(name) = self.stderr_panic.clone() else {
            return Vec::new();
        };
        let ends = line.starts_with("note: ")
            || line.starts_with("error: ")
            || line.trim_start().starts_with("Running ")
            || line.trim_start().starts_with("Doc-tests ");
        if ends {
            return self.end_stderr_panic();
        }
        if let Some(p) = self.panics.get_mut(&name) {
            if line == "stack backtrace:" {
                p.in_backtrace = true;
            }
            if p.in_backtrace {
                p.backtrace.push(line.to_owned());
            } else {
                p.message.push(line.to_owned());
            }
        }
        Vec::new()
    }

    /// The end of the output: what is still open is closed. Returns failed results whose panic arrived after them.
    pub fn finish_output(&mut self, now: Instant) -> Vec<RustResult> {
        let mut out = self.end_stderr_panic();
        if let Some((name, lines)) = self.section.take() {
            self.read_section(&name, &lines);
        }
        for (name, r) in std::mem::take(&mut self.failed) {
            if let Some(p) = self.panics.get(&name) {
                let mut r = r;
                p.apply(&mut r);
                out.push(r);
            }
        }
        if let Some((name, _, output)) = self.current.take() {
            let mut r = RustResult::new(&name, Outcome::NotRun);
            r.output = Some(output);
            r.duration_ms = self
                .started
                .get(&name)
                .map(|s| now.duration_since(*s).as_secs_f64() * 1e3);
            out.push(r);
        }
        out
    }

    fn end_stderr_panic(&mut self) -> Vec<RustResult> {
        let Some(name) = self.stderr_panic.take() else {
            return Vec::new();
        };
        // A test reported FAILED before its panic was read gets it now.
        match (self.failed.remove(&name), self.panics.get(&name)) {
            (Some(mut r), Some(p)) => {
                p.apply(&mut r);
                vec![r]
            }
            _ => Vec::new(),
        }
    }

    fn read_section(&mut self, name: &str, lines: &[String]) {
        // The test's captured stdout, then its panic (the harness prints it into the same capture).
        let mut output = Vec::new();
        let mut panic: Option<Panic> = None;
        for line in lines {
            if let Some((_, location)) = panic_header(line) {
                panic = Some(Panic {
                    location,
                    ..Panic::default()
                });
                continue;
            }
            match panic.as_mut() {
                Some(p) => {
                    if line.starts_with("note: ") {
                        continue;
                    }
                    if line == "stack backtrace:" {
                        p.in_backtrace = true;
                    }
                    if p.in_backtrace {
                        p.backtrace.push(line.clone());
                    } else {
                        p.message.push(line.clone());
                    }
                }
                None => output.push(line.clone()),
            }
        }
        let output = output.join("\n").trim().to_owned();
        if let Some(p) = panic {
            self.panics.insert(name.to_owned(), p);
        }
        if let Some(r) = self.failed.get_mut(name)
            && !output.is_empty()
        {
            r.output = Some(format!("{output}\n"));
        }
    }

    fn outcome(text: &str) -> Option<(Outcome, Option<String>)> {
        let text = text.trim();
        match text {
            "ok" => Some((Outcome::Passed, None)),
            "FAILED" => Some((Outcome::Failed, None)),
            "ignored" => Some((Outcome::Skipped, None)),
            t => t
                .strip_prefix("ignored, ")
                .map(|reason| (Outcome::Skipped, Some(reason.to_owned()))),
        }
    }

    fn finish(
        &mut self,
        name: &str,
        outcome: Outcome,
        reason: Option<String>,
        output: Option<String>,
        now: Instant,
    ) -> RustResult {
        let mut r = RustResult::new(name, outcome);
        r.duration_ms = self
            .started
            .get(name)
            .map(|s| now.duration_since(*s).as_secs_f64() * 1e3);
        r.message = reason;
        r.output = output.filter(|o| !o.trim().is_empty());
        if outcome == Outcome::Failed {
            if let Some(p) = self.panics.get(name) {
                p.apply(&mut r);
            }
            self.failed.insert(name.to_owned(), r.clone());
        }
        r
    }
}

/// What the Cargo test threads tell the UI.
#[derive(Debug, Clone)]
pub enum CargoTestEvent {
    /// A target's tests are listed (or could not be).
    Listed {
        ticket: u64,
        /// The package's `Cargo.toml`.
        package: PathBuf,
        target: TestTarget,
        /// `(name, source, line)`.
        tests: Vec<(String, Option<PathBuf>, Option<u32>)>,
        error: Option<String>,
    },
    /// Every target of the listing is done.
    ListingDone { ticket: u64 },
    /// A line for the Output window's Tests source.
    Output(String),
    /// Results of a run's target.
    Results {
        run: u64,
        package: PathBuf,
        target: TestTarget,
        results: Vec<RustResult>,
    },
    /// A target's process ended (`error` when it could not run or did not report).
    TargetDone {
        run: u64,
        package: PathBuf,
        target: TestTarget,
        error: Option<String>,
    },
    /// The whole Cargo part of a run ended.
    RunDone { run: u64, canceled: bool },
}

/// One package's targets to list or run.
#[derive(Debug, Clone)]
pub struct PackageJob {
    pub name: String,
    pub manifest: PathBuf,
    /// Targets and, for a run, the names to run in each (all when empty).
    pub targets: Vec<(TestTarget, Vec<String>)>,
}

/// A Cargo test run (or listing) in flight; `cancel` kills cargo's process group.
#[derive(Debug, Clone, Default)]
pub struct CargoTests {
    child: Arc<Mutex<Option<u32>>>,
    canceled: Arc<AtomicBool>,
}

impl CargoTests {
    pub fn cancel(&self) {
        self.canceled.store(true, Ordering::SeqCst);
        if let Some(pid) = *self.child.lock().unwrap_or_else(|e| e.into_inner()) {
            super::cargo_build::kill_tree(pid);
        }
    }

    fn canceled(&self) -> bool {
        self.canceled.load(Ordering::SeqCst)
    }
}

/// Where cargo runs and with what.
#[derive(Debug, Clone)]
pub struct CargoSetup {
    pub program: OsString,
    /// The workspace's `Cargo.toml` and root.
    pub manifest: PathBuf,
    pub root: PathBuf,
}

enum Line {
    Out(String),
    Err(String),
}

/// Run `cargo` with `args` in `root`, its own process group, reporting lines to `each` until it ends. Returns its exit
/// code, or why it could not start.
fn run_cargo(
    setup: &CargoSetup,
    args: &[String],
    handle: &CargoTests,
    mut each: impl FnMut(Line),
) -> Result<Option<i32>, String> {
    let mut cmd = Command::new(&setup.program);
    cmd.args(args)
        .current_dir(&setup.root)
        .env("RUST_BACKTRACE", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt as _;
        cmd.process_group(0);
    }
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("{} could not start: {e}", setup.program.to_string_lossy()))?;
    *handle.child.lock().unwrap_or_else(|e| e.into_inner()) = Some(child.id());
    if handle.canceled() {
        super::cargo_build::kill_tree(child.id());
    }
    let (tx, rx) = mpsc::channel::<Line>();
    let mut readers = Vec::new();
    if let Some(s) = child.stdout.take() {
        let tx = tx.clone();
        readers.push(thread::spawn(move || {
            for line in BufReader::new(s).lines().map_while(Result::ok) {
                if tx.send(Line::Out(line)).is_err() {
                    break;
                }
            }
        }));
    }
    if let Some(s) = child.stderr.take() {
        let tx = tx.clone();
        readers.push(thread::spawn(move || {
            for line in BufReader::new(s).lines().map_while(Result::ok) {
                if tx.send(Line::Err(line)).is_err() {
                    break;
                }
            }
        }));
    }
    drop(tx);
    for line in rx {
        each(line);
    }
    for r in readers {
        let _ = r.join();
    }
    let status = child.wait();
    *handle.child.lock().unwrap_or_else(|e| e.into_inner()) = None;
    Ok(status.ok().and_then(|s| s.code()))
}

fn command_line(setup: &CargoSetup, args: &[String]) -> String {
    format!("> {} {}\n", setup.program.to_string_lossy(), args.join(" "))
}

/// List the tests of `packages` on a thread; each target reports [`CargoTestEvent::Listed`], then
/// [`CargoTestEvent::ListingDone`].
pub fn list(
    ticket: u64,
    setup: CargoSetup,
    packages: Vec<PackageJob>,
    events: UnboundedSender<CargoTestEvent>,
) -> CargoTests {
    let handle = CargoTests::default();
    let h = handle.clone();
    let spawned = thread::Builder::new()
        .name("eludite-cargo-test-list".into())
        .spawn(move || {
            for p in packages {
                for (target, _) in p.targets {
                    if h.canceled() {
                        break;
                    }
                    let args = list_args(&setup.manifest, &p.name, &target);
                    let _ =
                        events.unbounded_send(CargoTestEvent::Output(command_line(&setup, &args)));
                    let mut stdout = String::new();
                    let mut stderr = String::new();
                    let code = run_cargo(&setup, &args, &h, |l| match l {
                        Line::Out(l) => {
                            stdout.push_str(&l);
                            stdout.push('\n');
                        }
                        Line::Err(l) => {
                            stderr.push_str(&l);
                            stderr.push('\n');
                        }
                    });
                    let (tests, error) = match code {
                        Ok(Some(0)) => (
                            parse_list(&stdout)
                                .into_iter()
                                .map(|name| {
                                    let found = find_test_fn(&target.src_path, &name);
                                    (
                                        name,
                                        found.as_ref().map(|f| f.0.clone()),
                                        found.map(|f| f.1),
                                    )
                                })
                                .collect(),
                            None,
                        ),
                        Ok(code) => {
                            let _ = events.unbounded_send(CargoTestEvent::Output(stderr.clone()));
                            (
                                Vec::new(),
                                Some(format!(
                                    "cargo test --list exited with {}",
                                    code.map_or("a signal".into(), |c| format!("code {c}"))
                                )),
                            )
                        }
                        Err(e) => (Vec::new(), Some(e)),
                    };
                    let _ = events.unbounded_send(CargoTestEvent::Listed {
                        ticket,
                        package: p.manifest.clone(),
                        target,
                        tests,
                        error,
                    });
                }
            }
            let _ = events.unbounded_send(CargoTestEvent::ListingDone { ticket });
        });
    if let Err(e) = spawned {
        eprintln!("eludite: cannot start the cargo test list thread: {e}");
    }
    handle
}

/// Run the named tests of `packages` on a thread, one `cargo test` per package and target, in order.
pub fn run(
    run: u64,
    setup: CargoSetup,
    packages: Vec<PackageJob>,
    parallel: bool,
    release: bool,
    events: UnboundedSender<CargoTestEvent>,
) -> CargoTests {
    let handle = CargoTests::default();
    let h = handle.clone();
    let spawned = thread::Builder::new()
        .name("eludite-cargo-test-run".into())
        .spawn(move || {
            for p in packages {
                for (target, names) in p.targets {
                    if h.canceled() {
                        break;
                    }
                    let args =
                        run_args(&setup.manifest, &p.name, &target, &names, parallel, release);
                    let _ =
                        events.unbounded_send(CargoTestEvent::Output(command_line(&setup, &args)));
                    let mut parser = Libtest::default();
                    let mut reported = false;
                    let send = |results: Vec<RustResult>, reported: &mut bool| {
                        if !results.is_empty() {
                            *reported = true;
                            let _ = events.unbounded_send(CargoTestEvent::Results {
                                run,
                                package: p.manifest.clone(),
                                target: target.clone(),
                                results,
                            });
                        }
                    };
                    let code = run_cargo(&setup, &args, &h, |l| {
                        let (text, results) = match l {
                            Line::Out(l) => {
                                let r = parser.stdout(&l, Instant::now());
                                (l, r)
                            }
                            Line::Err(l) => {
                                let r = parser.stderr(&l);
                                (l, r)
                            }
                        };
                        let _ = events.unbounded_send(CargoTestEvent::Output(format!("{text}\n")));
                        send(results, &mut reported);
                    });
                    send(parser.finish_output(Instant::now()), &mut reported);
                    let error = match code {
                        Err(e) => Some(e),
                        _ if h.canceled() => None,
                        Ok(Some(0 | 101)) if reported => None,
                        Ok(code) => Some(format!(
                            "cargo test exited with {}",
                            code.map_or("a signal".into(), |c| format!("code {c}"))
                        )),
                    };
                    let _ = events.unbounded_send(CargoTestEvent::TargetDone {
                        run,
                        package: p.manifest.clone(),
                        target: target.clone(),
                        error,
                    });
                }
            }
            let _ = events.unbounded_send(CargoTestEvent::RunDone {
                run,
                canceled: h.canceled(),
            });
        });
    if let Err(e) = spawned {
        eprintln!("eludite: cannot start the cargo test thread: {e}");
    }
    handle
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn lib() -> TestTarget {
        TestTarget {
            kind: TargetKind::Lib,
            name: "corpus_tests".into(),
            src_path: "/w/src/lib.rs".into(),
        }
    }

    #[test]
    fn arguments_list_and_run_a_target() {
        let integration = TestTarget {
            kind: TargetKind::Test,
            name: "integration".into(),
            src_path: "/w/tests/integration.rs".into(),
        };
        assert_eq!(lib().key(), "lib");
        assert_eq!(integration.key(), "test:integration");
        assert_eq!(integration.prefix(), Some("integration"));
        assert_eq!(
            list_args(Path::new("/w/Cargo.toml"), "corpus-tests", &integration),
            [
                "test",
                "--manifest-path",
                "/w/Cargo.toml",
                "-p",
                "corpus-tests",
                "--test",
                "integration",
                "--",
                "--list",
                "--format",
                "terse"
            ]
        );
        assert_eq!(
            run_args(
                Path::new("/w/Cargo.toml"),
                "corpus-tests",
                &lib(),
                &["tests::adds".into()],
                false,
                false
            )[5..],
            [
                "--lib",
                "--",
                "--exact",
                "tests::adds",
                "--nocapture",
                "--test-threads=1"
            ]
        );
        let parallel = run_args(Path::new("/w/Cargo.toml"), "p", &lib(), &[], true, true);
        assert_eq!(parallel[5..], ["--lib", "--release", "--"]);
        assert_eq!(
            parse_list("tests::adds: test\ntests::nested::adds_negatives: test\nbench_x: bench\n"),
            ["tests::adds", "tests::nested::adds_negatives"]
        );
    }

    #[test]
    fn libtest_nocapture_output_is_parsed_into_results() {
        let t0 = Instant::now();
        let at = |ms: u64| t0 + Duration::from_millis(ms);
        let mut p = Libtest::default();
        let mut all = Vec::new();
        for (i, line) in [
            "",
            "running 4 tests",
            "test tests::adds ... ok",
            "test tests::divides ... ignored, division is not written yet",
            "test tests::subtracts ... FAILED",
            "test tests::writes_output ... Hello from Rust",
            "second line",
            "ok",
        ]
        .into_iter()
        .enumerate()
        {
            all.extend(p.stdout(line, at(i as u64 * 10)));
        }
        // The panic arrives on stderr after the FAILED line.
        for line in [
            "     Running unittests src/lib.rs (target/debug/deps/corpus_tests-1)",
            "",
            "thread 'tests::subtracts' (6228) panicked at src/lib.rs:25:9:",
            "assertion `left == right` failed",
            "  left: -1",
            " right: 1",
            "stack backtrace:",
            "   4: corpus_tests::tests::subtracts",
            "             at ./src/lib.rs:25:9",
            "note: Some details are omitted, run with `RUST_BACKTRACE=full` for a verbose backtrace.",
        ] {
            all.extend(p.stderr(line));
        }
        all.extend(p.finish_output(at(100)));
        let get = |name: &str| {
            all.iter()
                .rev()
                .find(|r| r.name == name)
                .cloned()
                .unwrap_or_else(|| panic!("{name}: {all:?}"))
        };
        assert_eq!(get("tests::adds").outcome, Outcome::Passed);
        let ignored = get("tests::divides");
        assert_eq!(ignored.outcome, Outcome::Skipped);
        assert_eq!(
            ignored.message.as_deref(),
            Some("division is not written yet")
        );
        let failed = get("tests::subtracts");
        assert_eq!(failed.outcome, Outcome::Failed);
        assert_eq!(
            failed.message.as_deref(),
            Some("assertion `left == right` failed\n  left: -1\n right: 1")
        );
        assert_eq!(failed.location, Some(("src/lib.rs".into(), 25)));
        assert!(failed.stack_trace.unwrap().contains("./src/lib.rs:25:9"));
        let output = get("tests::writes_output");
        assert_eq!(output.outcome, Outcome::Passed);
        assert_eq!(
            output.output.as_deref(),
            Some("Hello from Rust\nsecond line\n")
        );
        assert_eq!(output.duration_ms, Some(20.0));
        // The test that printed was reported running first.
        assert!(
            all.iter()
                .any(|r| r.name == "tests::writes_output" && r.outcome == Outcome::Running)
        );
    }

    #[test]
    fn libtest_captured_sections_and_unfinished_tests() {
        let now = Instant::now();
        let mut p = Libtest::default();
        let mut all = Vec::new();
        for line in [
            "running 2 tests",
            "test tests::writes_output ... ok",
            "test tests::subtracts ... FAILED",
            "",
            "failures:",
            "",
            "---- tests::subtracts stdout ----",
            "printed before",
            "",
            "thread 'tests::subtracts' panicked at src/lib.rs:25:9:",
            "assertion failed",
            "note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace",
            "",
            "",
            "failures:",
            "    tests::subtracts",
            "",
            "test result: FAILED. 1 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out",
        ] {
            all.extend(p.stdout(line, now));
        }
        all.extend(p.finish_output(now));
        let failed = all
            .iter()
            .rev()
            .find(|r| r.name == "tests::subtracts")
            .unwrap();
        assert_eq!(failed.message.as_deref(), Some("assertion failed"));
        assert_eq!(failed.output.as_deref(), Some("printed before\n"));
        assert_eq!(failed.stack_trace.as_deref(), Some("at src/lib.rs:25"));
        // A test still printing when the process died is not run.
        let mut p = Libtest::default();
        p.stdout("test tests::waits ... started", now);
        let open = p.finish_output(now);
        assert_eq!(open[0].outcome, Outcome::NotRun);
    }

    #[test]
    fn test_functions_are_found_in_the_sources() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("src");
        std::fs::create_dir_all(src.join("deep")).unwrap();
        std::fs::write(
            src.join("lib.rs"),
            "mod deep;\nfn helper() {}\n#[cfg(test)]\nmod tests {\n    #[test]\n    fn adds() {}\n}\n",
        )
        .unwrap();
        std::fs::write(
            src.join("deep/inner.rs"),
            "#[test]\n#[should_panic]\nfn adds() {}\n",
        )
        .unwrap();
        let lib = src.join("lib.rs");
        assert_eq!(find_test_fn(&lib, "tests::adds"), Some((lib.clone(), 6)));
        assert_eq!(
            find_test_fn(&lib, "deep::inner::adds"),
            Some((lib.clone(), 6)),
            "the root file comes first: a limit of the name match"
        );
        assert_eq!(find_test_fn(&lib, "tests::helper"), None);
        assert_eq!(
            panic_header("thread 'a::b' (12) panicked at src/x.rs:3:4:"),
            Some(("a::b".into(), Some(("src/x.rs".into(), 3))))
        );
        assert_eq!(
            panic_header("thread 'main' panicked at x"),
            Some(("main".into(), None))
        );
    }
}
