//! The Test Explorer's model (brief 0035): the discovered tests of the workspace (.NET containers from `eludite-host`,
//! Cargo packages from `cargo_tests`), the runs and their results, the `eludite.test.*` commands, the Error List's
//! test rows, the status bar's tests slot, and Debug Test.
//!
//! - **One path for everyone.** The window's buttons, the Test menu, the keys and agents run the same commands. On the
//!   UI thread the shell applies a request and stages the result for the bus handler; from another thread the request
//!   is posted to the UI as a [`TestJob`], and the calling thread then waits on [`TestShared`] for the run, the
//!   discovery or the debugging session, never the UI thread.
//! - **Discovery** is kept per solution generation and per build: a build that is not the Test Explorer's own makes it
//!   stale. Discovering builds first when it is stale (or `rebuild`): the .NET solution through `eludite.build.*`'s
//!   path and the Cargo workspace with `cargo test --no-run`, both into the Build pane and the Error List; then the
//!   host discovers the containers (`eludite/test/discover`) and the shell lists the Cargo targets.
//! - **Runs** start when the tree is ready (discovering first when it is not). .NET tests run through
//!   `eludite/test/run`, Rust tests through `cargo test`; results stream into the tree, the run's record and the window.
//!   A run ends when both parts ended: the Error List gets a row per failed test (at the failure's first stack frame in
//!   the project, else the test's declaration) and the status bar reads `Tests: 12 passed, 1 failed, 2 skipped (3.2 s)`.
//! - **The generation rule.** Updates computed under an older solution generation are dropped (CLAUDE.md invariant
//!   12); a new generation forgets the .NET tests. After a host restart `eludite/test/status` replays what is going.
//! - **Debug Test** builds, then starts a brief 0028 session named after the project: an MTP test application under
//!   its adapter from the host's `launch` update (the debugger's start seam, `Debugger::test_launch`), the VSTest
//!   testhost attached to from the host's `attach` update, or a Cargo package's test executable (`test: true`). A
//!   temporary function breakpoint on the first test (`Namespace.Class.Method`, `crate::module::test`) stops it at the
//!   test's first line.

use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Condvar, Mutex, Weak};
use std::thread::ThreadId;
use std::time::{Duration, Instant};

use eludite_commands::build::{BuildKind, BuildRequest, BuildSystemChoice, OutputSource};
use eludite_commands::test::{
    self as cmds, CancelOutput, DebugOutput, DiscoverCounts, DiscoverOutput, DiscoverProject,
    DiscoverTest, Outcome, ResultRow, ResultsOutput, RunOutput, RunSummary, Selection,
    TestCommands, TestOutput, TestRequest,
};
use eludite_commands::view::{ViewRequest, ViewTarget as _};
use eludite_commands::{CommandError, CommandRegistry};
use eludite_docking::ids;
use eludite_lsp::host::{
    self, TestContainer, TestItem, TestLaunch, TestOutcome, TestProtocol, TestResultItem,
    TestRuntime, TestState, TestStatusResult, TestUpdate, TestUpdateKind,
};
use futures::channel::mpsc::UnboundedSender;
use gpui::{Context, Window};
use serde_json::{Value, json};

use super::Shell;
use super::cargo_tests::{self, CargoSetup, CargoTestEvent, CargoTests, PackageJob, TestTarget};
use super::debug::PrelaunchBuild;

/// Status bar slot: the last test run's summary (left, after the build's).
pub const TESTS_SLOT: &str = "tests";

/// Whether menu item `command` is enabled given whether a test run goes: Run All Tests, Debug All Tests and Repeat
/// Last Run wait for it to end.
pub fn menu_enabled(command: &str, running: bool) -> bool {
    !(running && (command == cmds::RUN || command == cmds::DEBUG))
}

/// How long a command from another thread waits for the UI thread to take it.
const UI_TIMEOUT: Duration = Duration::from_secs(30);

/// Runs kept for `eludite.test.results`.
const KEPT_RUNS: usize = 20;

type Outcome2 = Result<TestOutput, CommandError>;

thread_local! {
    static STAGED: RefCell<Option<Outcome2>> = const { RefCell::new(None) };
}

/// The result the shell computed for the bus invocation it is about to make on this (the UI) thread.
pub fn stage(outcome: Outcome2) {
    STAGED.with(|s| *s.borrow_mut() = Some(outcome));
}

/// What the off-UI caller waits for after its request was applied.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ticket {
    /// A discovery: the shared discovery counter reaching this.
    Discover(u64),
    Run(u64),
    Debug(u64),
}

/// A call from another thread for the UI thread.
#[derive(Debug, Clone)]
pub enum TestCall {
    Apply(TestRequest),
    /// The answer of a run or discovery the caller waited for.
    Answer {
        request: TestRequest,
        ticket: Ticket,
        timed_out: bool,
    },
    /// A debugged run's answer once its session stopped (or the wait ended).
    DebugAnswer {
        run: u64,
        summary: Value,
    },
}

/// A test command from another thread, for the UI thread to apply.
pub struct TestJob {
    pub call: TestCall,
    pub reply: mpsc::SyncSender<(Outcome2, Option<Ticket>)>,
}

/// What a waiting command reads.
#[derive(Default)]
pub struct TestShared {
    state: Mutex<SharedState>,
    changed: Condvar,
}

#[derive(Default)]
struct SharedState {
    discoveries: u64,
    finished_runs: HashSet<u64>,
    /// Debugged runs: the session once it started, or why it could not.
    debug: HashMap<u64, Result<u32, String>>,
}

impl TestShared {
    fn update(&self, f: impl FnOnce(&mut SharedState)) {
        let mut s = self.state.lock().unwrap_or_else(|e| e.into_inner());
        f(&mut s);
        self.changed.notify_all();
    }

    fn wait_for(&self, timeout: Duration, mut done: impl FnMut(&SharedState) -> bool) -> bool {
        let deadline = Instant::now() + timeout;
        let mut s = self.state.lock().unwrap_or_else(|e| e.into_inner());
        loop {
            if done(&s) {
                return true;
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return false;
            }
            s = self
                .changed
                .wait_timeout(s, left)
                .unwrap_or_else(|e| e.into_inner())
                .0;
        }
    }

    fn debug_session(&self, run: u64) -> Option<Result<u32, String>> {
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .debug
            .get(&run)
            .cloned()
    }
}

/// The shell's [`TestCommands`].
pub struct TestBus {
    pub ui_thread: ThreadId,
    pub jobs: UnboundedSender<TestJob>,
    pub shared: Arc<TestShared>,
    /// The bus itself, for a debugged run's `eludite.debug.wait` (set once the registry is shared).
    pub registry: Arc<Mutex<Weak<CommandRegistry>>>,
}

impl TestBus {
    fn post(&self, call: TestCall) -> Result<(Outcome2, Option<Ticket>), CommandError> {
        let (reply, rx) = mpsc::sync_channel(1);
        self.jobs
            .unbounded_send(TestJob { call, reply })
            .map_err(|_| CommandError::Failed("the window is closed".into()))?;
        rx.recv_timeout(UI_TIMEOUT)
            .map_err(|_| CommandError::Failed("the UI did not answer".into()))
    }
}

impl TestCommands for TestBus {
    fn apply(&self, request: TestRequest) -> Outcome2 {
        if std::thread::current().id() == self.ui_thread {
            return STAGED.with(|s| s.borrow_mut().take()).unwrap_or_else(|| {
                Err(CommandError::Failed(format!(
                    "{} runs on the UI thread through the shell",
                    request.command()
                )))
            });
        }
        let started = Instant::now();
        let wait = Duration::from_millis(request.agent_wait_ms().unwrap_or(0));
        let (outcome, ticket) = self.post(TestCall::Apply(request.clone()))?;
        let Some(ticket) = ticket.filter(|_| outcome.is_ok() && !wait.is_zero()) else {
            return outcome;
        };
        match ticket {
            Ticket::Discover(n) => {
                let done = self.shared.wait_for(wait, |s| s.discoveries >= n);
                self.post(TestCall::Answer {
                    request,
                    ticket,
                    timed_out: !done,
                })?
                .0
            }
            Ticket::Run(run) => {
                let done = self
                    .shared
                    .wait_for(wait, |s| s.finished_runs.contains(&run));
                self.post(TestCall::Answer {
                    request,
                    ticket,
                    timed_out: !done,
                })?
                .0
            }
            Ticket::Debug(run) => {
                self.shared.wait_for(wait, |s| {
                    s.debug.contains_key(&run) || s.finished_runs.contains(&run)
                });
                let left = wait.saturating_sub(started.elapsed());
                let summary = match self.shared.debug_session(run) {
                    Some(Ok(session)) => {
                        let registry = self
                            .registry
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .upgrade();
                        let TestRequest::Debug { budget, .. } = &request else {
                            unreachable!("a debug ticket answers a debug request")
                        };
                        let mut args = json!({
                            "session": session,
                            "until": "stopped",
                            "wait_ms": left.as_millis().clamp(1, 30_000) as u64,
                        });
                        for (k, v) in budget {
                            args[k] = v.clone();
                        }
                        match registry {
                            Some(r) => r
                                .invoke(eludite_commands::debug::WAIT, args)
                                .unwrap_or_else(
                                    |e| json!({"mode": "design", "message": e.to_string()}),
                                ),
                            None => json!({"mode": "launching"}),
                        }
                    }
                    Some(Err(why)) => json!({"mode": "design", "message": why}),
                    None => json!({"mode": "launching", "timed_out": true}),
                };
                self.post(TestCall::DebugAnswer { run, summary })?.0
            }
        }
    }
}

/// Which protocol a project speaks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Proto {
    Mtp,
    Vstest,
    Cargo,
}

impl Proto {
    pub fn as_str(self) -> &'static str {
        match self {
            Proto::Mtp => "mtp",
            Proto::Vstest => "vstest",
            Proto::Cargo => "cargo",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeState {
    Discovering,
    Ready,
    Failed,
}

/// A project node of the tree: a .NET container (a test project for one target framework) or a Cargo package.
#[derive(Debug, Clone, PartialEq)]
pub struct ProjectNode {
    /// The container id, or the package's `Cargo.toml`.
    pub key: String,
    pub name: String,
    /// The project file or `Cargo.toml`.
    pub path: String,
    pub protocol: Proto,
    pub target_framework: Option<String>,
    pub runtime: Option<TestRuntime>,
    pub program: Option<String>,
    pub state: NodeState,
    pub message: Option<String>,
    /// A Cargo package's name and test targets.
    pub cargo: Option<(String, Vec<TestTarget>)>,
}

/// A test's result in a run.
#[derive(Debug, Clone, PartialEq)]
pub struct ResultData {
    pub outcome: Outcome,
    pub duration_ms: Option<f64>,
    pub message: Option<String>,
    pub stack_trace: Option<String>,
    pub output: Option<String>,
    /// Where the failure happened (its first stack frame in the project).
    pub failure: Option<(PathBuf, u32)>,
}

/// A test of the tree.
#[derive(Debug, Clone, PartialEq)]
pub struct TestNode {
    /// `<project key>|<runner id>`.
    pub id: String,
    /// The runner's id: MTP's uid, VSTest's TestCase id, `<target key>|<libtest name>` for Cargo.
    pub runner_id: String,
    pub project: String,
    /// As the tree shows it (the method, or a data row's display name).
    pub name: String,
    pub full_name: String,
    pub group: Vec<String>,
    pub source: Option<String>,
    pub line: Option<u32>,
    pub traits: Vec<(String, String)>,
    /// The last run's result for it.
    pub result: Option<ResultData>,
    /// A Cargo test's target and libtest name.
    pub cargo: Option<(TestTarget, String)>,
}

impl TestNode {
    pub fn outcome(&self) -> Outcome {
        self.result.as_ref().map_or(Outcome::NotRun, |r| r.outcome)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunState {
    Building,
    Discovering,
    Running,
    Passed,
    Failed,
    Canceled,
}

impl RunState {
    pub fn as_str(self) -> &'static str {
        match self {
            RunState::Building => "building",
            RunState::Discovering => "discovering",
            RunState::Running => "running",
            RunState::Passed => "passed",
            RunState::Failed => "failed",
            RunState::Canceled => "canceled",
        }
    }

    pub fn done(self) -> bool {
        matches!(
            self,
            RunState::Passed | RunState::Failed | RunState::Canceled
        )
    }
}

/// How a debugged run reaches its session.
#[derive(Debug, Clone, PartialEq)]
pub enum DebugKind {
    /// .NET: the host's `launch` or `attach` update starts it.
    Host,
    /// A Cargo package's test executable (`eludite.debug.start` with `test: true`).
    Cargo {
        manifest: String,
        target: Option<String>,
        names: Vec<String>,
    },
}

/// A debugged run's state beyond its record.
#[derive(Debug, Clone, PartialEq)]
pub struct DebugRun {
    pub kind: DebugKind,
    pub project: String,
    pub breakpoint: Option<String>,
    /// The session once it started.
    pub session: Option<u32>,
    /// The `attach` update's process, until the shell answered `eludite/test/attached`.
    pub attaching: Option<u32>,
    /// Debug output lines already parsed (a Cargo run's results are read from the Debug pane).
    pub output_from: usize,
}

/// One run.
#[derive(Debug, Clone)]
pub struct RunRecord {
    pub id: u64,
    pub state: RunState,
    pub selection: Selection,
    /// The tests it covers (resolved once the tree is ready).
    pub tests: Vec<String>,
    pub results: HashMap<String, ResultData>,
    pub started: Instant,
    pub elapsed: Option<Duration>,
    pub host_run: Option<u64>,
    pub host_pending: bool,
    pub cargo_pending: bool,
    pub canceled: bool,
    pub message: Option<String>,
    pub debug: Option<DebugRun>,
}

impl RunRecord {
    pub fn summary(&self) -> RunSummary {
        let mut s = RunSummary {
            total: self.tests.len() as u32,
            ..RunSummary::default()
        };
        for id in &self.tests {
            match self.results.get(id).map(|r| r.outcome) {
                Some(Outcome::Passed) => s.passed += 1,
                Some(Outcome::Failed) => s.failed += 1,
                Some(Outcome::Skipped) => s.skipped += 1,
                _ => s.not_run += 1,
            }
        }
        s.duration_ms = Some(
            self.elapsed
                .unwrap_or_else(|| self.started.elapsed())
                .as_secs_f64()
                * 1e3,
        );
        s
    }
}

/// The discovery's phase.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    Idle,
    Building,
    Discovering,
    Ready,
    Failed,
}

/// When the steps of the last discovery and run happened (the brief 0035 report).
#[derive(Debug, Default, Clone)]
#[allow(dead_code)]
pub struct TestTimings {
    pub discover_started: Option<Instant>,
    pub discover_done: Option<Instant>,
    /// The window's tree was last rebuilt, and how long that took.
    pub tree_built: Option<Duration>,
    /// The first result of the last run was received (by the pump or the cargo thread's channel), and when the window
    /// had its row.
    pub first_result_received: Option<Instant>,
    pub first_result_shown: Option<Instant>,
    pub run_started: Option<Instant>,
    pub run_done: Option<Instant>,
}

/// The Test Explorer's state (see the module docs).
pub struct TestRuns {
    pub shared: Arc<TestShared>,
    /// True while a run goes: the Test menu's start items read it.
    pub running: Arc<AtomicBool>,
    /// The solution generation the .NET part belongs to.
    pub generation: u64,
    pub projects: Vec<ProjectNode>,
    pub tests: Vec<TestNode>,
    index: HashMap<String, usize>,
    pub phase: Phase,
    /// Discovered under the current generation and since the last build.
    pub fresh: bool,
    pub phase_message: Option<String>,
    /// The build the discovery waits for.
    build_ticket: Option<u64>,
    /// The host's discovery and the Cargo listing still going.
    host_discovery: Option<u64>,
    host_pending: bool,
    cargo_listing: Option<u64>,
    next_listing: u64,
    pub runs: VecDeque<RunRecord>,
    pub current: Option<u64>,
    next_run: u64,
    /// A run waiting for the discovery.
    waiting: Option<u64>,
    /// The Cargo part of the run that goes, to cancel it.
    cargo: Option<CargoTests>,
    cargo_list: Option<CargoTests>,
    /// The setting test.parallel, test.runSettings and test.vstestConsolePath.
    pub parallel: bool,
    pub run_settings: Option<String>,
    pub vstest_console: Option<String>,
    pub timings: TestTimings,
    /// The window needs the tree again.
    pub dirty: bool,
    /// The build a debugged run waits for.
    pending_debug_build: Option<u64>,
    /// Updates of a discovery or run whose reply has not arrived yet (the host streams right after answering, and
    /// the reply and the updates reach the UI by different paths): applied once its id is known.
    early: Vec<TestUpdate>,
}

impl TestRuns {
    pub fn new(shared: Arc<TestShared>) -> Self {
        Self {
            shared,
            running: Arc::default(),
            generation: 0,
            projects: Vec::new(),
            tests: Vec::new(),
            index: HashMap::new(),
            phase: Phase::Idle,
            fresh: false,
            phase_message: None,
            build_ticket: None,
            host_discovery: None,
            host_pending: false,
            cargo_listing: None,
            next_listing: 1,
            runs: VecDeque::new(),
            current: None,
            next_run: 1,
            waiting: None,
            cargo: None,
            cargo_list: None,
            parallel: false,
            run_settings: None,
            vstest_console: None,
            timings: TestTimings::default(),
            dirty: true,
            pending_debug_build: None,
            early: Vec::new(),
        }
    }

    pub fn run(&self, id: u64) -> Option<&RunRecord> {
        self.runs.iter().find(|r| r.id == id)
    }

    fn run_mut(&mut self, id: u64) -> Option<&mut RunRecord> {
        self.runs.iter_mut().find(|r| r.id == id)
    }

    pub fn last_run(&self) -> Option<&RunRecord> {
        self.runs.back()
    }

    pub fn test(&self, id: &str) -> Option<&TestNode> {
        self.index.get(id).map(|&i| &self.tests[i])
    }

    fn project(&self, key: &str) -> Option<&ProjectNode> {
        self.projects.iter().find(|p| p.key == key)
    }

    fn reindex(&mut self) {
        self.index = self
            .tests
            .iter()
            .enumerate()
            .map(|(i, t)| (t.id.clone(), i))
            .collect();
        self.dirty = true;
    }

    /// Replace the projects of `proto` kinds (keeping the others and their tests).
    fn replace_projects(&mut self, cargo: bool, projects: Vec<ProjectNode>) {
        let keys: HashSet<String> = self
            .projects
            .iter()
            .filter(|p| (p.protocol == Proto::Cargo) == cargo)
            .map(|p| p.key.clone())
            .collect();
        self.tests.retain(|t| !keys.contains(&t.project));
        self.projects
            .retain(|p| (p.protocol == Proto::Cargo) != cargo);
        if cargo {
            self.projects.extend(projects);
        } else {
            let mut list = projects;
            list.append(&mut self.projects);
            self.projects = list;
        }
        self.reindex();
    }

    fn add_test(&mut self, node: TestNode) {
        if let Some(&i) = self.index.get(&node.id) {
            self.tests[i] = node;
        } else {
            self.index.insert(node.id.clone(), self.tests.len());
            self.tests.push(node);
        }
        self.dirty = true;
    }

    /// The tests matching a filter: a trait `Name=Value`, else a case-insensitive substring of the full name.
    pub fn matches(t: &TestNode, filter: &str) -> bool {
        if let Some((name, value)) = filter.split_once('=') {
            let (name, value) = (name.trim(), value.trim());
            return t
                .traits
                .iter()
                .any(|(n, v)| n.eq_ignore_ascii_case(name) && v.eq_ignore_ascii_case(value));
        }
        let f = filter.to_lowercase();
        t.full_name.to_lowercase().contains(&f) || t.name.to_lowercase().contains(&f)
    }

    /// A project named as the commands take it: the Test Explorer's name, the name without a target framework, the
    /// project file or Cargo.toml (absolute or relative to `root`), or a Cargo package's name.
    pub fn projects_named(&self, project: &str, root: Option<&Path>) -> Vec<String> {
        let as_path = {
            let p = Path::new(project);
            if p.is_absolute() {
                Some(p.to_path_buf())
            } else {
                root.map(|r| r.join(p))
            }
        };
        let norm = |p: &Path| super::documents::normalize_path(p);
        self.projects
            .iter()
            .filter(|p| {
                p.name == project
                    || p.name.split(" (").next() == Some(project)
                    || p.cargo.as_ref().is_some_and(|(n, _)| n == project)
                    || as_path
                        .as_deref()
                        .is_some_and(|a| norm(a) == norm(Path::new(&p.path)))
            })
            .map(|p| p.key.clone())
            .collect()
    }

    fn set_discovery_done(&mut self) {
        if self.host_pending || self.cargo_listing.is_some() || self.phase != Phase::Discovering {
            return;
        }
        let failed =
            !self.projects.is_empty() && self.projects.iter().all(|p| p.state == NodeState::Failed);
        self.phase = if failed { Phase::Failed } else { Phase::Ready };
        if failed && self.phase_message.is_none() {
            self.phase_message = self.projects.iter().find_map(|p| p.message.clone());
        }
        self.fresh = true;
        self.timings.discover_done = Some(Instant::now());
        self.shared.update(|s| s.discoveries += 1);
        self.dirty = true;
    }
}

/// The `in <file>:line <n>` frames of a .NET stack trace, in order.
pub fn stack_frames(stack: &str) -> Vec<(PathBuf, u32)> {
    stack
        .lines()
        .filter_map(|l| {
            let (_, rest) = l.split_once(" in ")?;
            let (file, line) = rest.rsplit_once(":line ")?;
            Some((PathBuf::from(file.trim()), line.trim().parse().ok()?))
        })
        .collect()
}

/// The first frame of `stack` under `dir` (the project's folder), else the first frame.
pub fn first_frame_in(stack: &str, dir: Option<&Path>) -> Option<(PathBuf, u32)> {
    let frames = stack_frames(stack);
    frames
        .iter()
        .find(|(f, _)| dir.is_some_and(|d| f.starts_with(d)))
        .or(frames.first())
        .cloned()
}

fn outcome_of(o: TestOutcome) -> Outcome {
    match o {
        TestOutcome::Running => Outcome::Running,
        TestOutcome::Passed => Outcome::Passed,
        TestOutcome::Failed => Outcome::Failed,
        TestOutcome::Skipped => Outcome::Skipped,
        TestOutcome::NotRun => Outcome::NotRun,
    }
}

/// `n` tests as the summary words it.
pub fn summary_text(s: &RunSummary) -> String {
    let mut parts = vec![
        format!("{} passed", s.passed),
        format!("{} failed", s.failed),
    ];
    if s.skipped > 0 {
        parts.push(format!("{} skipped", s.skipped));
    }
    if s.not_run > 0 {
        parts.push(format!("{} not run", s.not_run));
    }
    format!(
        "Tests: {} ({:.1} s)",
        parts.join(", "),
        s.duration_ms.unwrap_or(0.) / 1e3
    )
}

/// A .NET test from the host's model.
fn dotnet_node(container: &str, item: &TestItem) -> TestNode {
    let mut group = Vec::new();
    if let Some(ns) = item.namespace.as_ref().filter(|n| !n.is_empty()) {
        group.push(ns.clone());
    }
    if let Some(c) = &item.class_name {
        group.push(c.clone());
    }
    // A data row shows its display name without the class prefix; a plain test its method.
    let prefix = item
        .fully_qualified_name
        .rsplit_once('.')
        .map(|(p, _)| format!("{p}."));
    let name = match &item.method {
        Some(m) if item.display_name == item.fully_qualified_name || item.display_name == *m => {
            m.clone()
        }
        _ => prefix
            .as_deref()
            .and_then(|p| item.display_name.strip_prefix(p))
            .unwrap_or(&item.display_name)
            .to_owned(),
    };
    TestNode {
        id: format!("{container}|{}", item.id),
        runner_id: item.id.clone(),
        project: container.to_owned(),
        name,
        full_name: item.fully_qualified_name.clone(),
        group,
        source: item.source.clone(),
        line: item.line,
        traits: item
            .traits
            .iter()
            .map(|t| (t.name.clone(), t.value.clone()))
            .collect(),
        result: None,
        cargo: None,
    }
}

fn project_of_container(c: &TestContainer) -> ProjectNode {
    ProjectNode {
        key: c.id.clone(),
        name: c.name.clone(),
        path: c.project.clone(),
        protocol: match c.protocol {
            TestProtocol::Mtp => Proto::Mtp,
            TestProtocol::Vstest => Proto::Vstest,
        },
        target_framework: Some(c.target_framework.clone()),
        runtime: c.runtime,
        program: c.program.clone(),
        state: if c.error.is_some() {
            NodeState::Failed
        } else {
            NodeState::Discovering
        },
        message: c.error.clone(),
        cargo: None,
    }
}

impl Shell {
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn test_runs(&self) -> &TestRuns {
        &self.tests
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn tests_window(&self) -> &gpui::Entity<super::tests_window::TestExplorer> {
        &self.tests_window
    }

    /// The settings test.parallel, test.runSettings (relative to the workspace's folder) and test.vstestConsolePath.
    pub(super) fn apply_test_settings(&mut self) {
        let (parallel, settings, console) = {
            let s = self.settings.lock();
            (
                s.bool("test.parallel"),
                s.string("test.runSettings"),
                s.path("test.vstestConsolePath"),
            )
        };
        self.tests.parallel = parallel;
        self.tests.run_settings = (!settings.trim().is_empty()).then(|| {
            let p = Path::new(settings.trim());
            match self.workspace_root() {
                Some(root) if p.is_relative() => root.join(p).to_string_lossy().into_owned(),
                _ => p.to_string_lossy().into_owned(),
            }
        });
        self.tests.vstest_console = console.map(|p| p.to_string_lossy().into_owned());
    }

    /// Apply a test command on the UI thread. The ticket says what an off-UI caller waits for.
    pub(super) fn apply_test(
        &mut self,
        request: TestRequest,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> (Outcome2, Option<Ticket>) {
        let out = match request.clone() {
            TestRequest::Explorer { filter } => (self.show_test_explorer(filter, window, cx), None),
            TestRequest::Discover { rebuild, .. } => {
                let started = self.test_discover(rebuild, window, cx);
                let ticket = started.map(Ticket::Discover);
                (
                    Ok(TestOutput::Discover(Box::new(
                        self.discover_output(&request),
                    ))),
                    ticket,
                )
            }
            TestRequest::Run { selection, .. } => {
                match self.test_run(selection, None, window, cx) {
                    Ok(id) => (
                        Ok(TestOutput::Run(Box::new(self.run_output(id, false)))),
                        Some(Ticket::Run(id)),
                    ),
                    Err(e) => (Err(e), None),
                }
            }
            TestRequest::Debug { selection, .. } => match self.test_debug(selection, window, cx) {
                Ok(id) => (
                    Ok(TestOutput::Debug(Box::new(self.debug_output(id, None, cx)))),
                    Some(Ticket::Debug(id)),
                ),
                Err(e) => (Err(e), None),
            },
            TestRequest::Results { .. } => (self.results_output(&request), None),
            TestRequest::Cancel { run } => {
                (Ok(TestOutput::Cancel(self.test_cancel(run, cx))), None)
            }
        };
        cx.notify();
        out
    }

    /// The answer of an off-UI call once it waited.
    pub(super) fn answer_test(&mut self, call: TestCall, cx: &mut Context<Self>) -> Outcome2 {
        match call {
            TestCall::Answer {
                request,
                ticket: Ticket::Run(id),
                timed_out,
            } => {
                let _ = request;
                Ok(TestOutput::Run(Box::new(self.run_output(id, timed_out))))
            }
            TestCall::Answer { request, .. } => Ok(TestOutput::Discover(Box::new(
                self.discover_output(&request),
            ))),
            TestCall::DebugAnswer { run, summary } => Ok(TestOutput::Debug(Box::new(
                self.debug_output(run, Some(summary), cx),
            ))),
            TestCall::Apply(_) => unreachable!("applied by apply_test"),
        }
    }

    /// Test > Test Explorer: show the window, set its search box, discover the outputs as they are when nothing was
    /// discovered yet.
    pub(super) fn show_test_explorer(
        &mut self,
        filter: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Outcome2 {
        let place = self.controller.apply(ViewRequest::Show {
            id: ids::TEST_EXPLORER.into(),
        })?;
        if let Some(f) = filter {
            self.tests_window.update(cx, |w, cx| w.set_filter(f, cx));
        }
        if self.tests.phase == Phase::Idle
            && (self.solution.is_some() || self.cargo_workspace().is_some())
        {
            self.test_discover(Some(false), window, cx);
        }
        Ok(TestOutput::Explorer(place.to_json()))
    }

    /// Start a discovery (building first when the kept one is stale, or always with `rebuild`). Returns the discovery
    /// counter value that marks it done, or `None` when the kept discovery answers.
    pub(super) fn test_discover(
        &mut self,
        rebuild: Option<bool>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<u64> {
        let has_dotnet = self.solution.is_some();
        let has_cargo = self.cargo_workspace().is_some();
        if !has_dotnet && !has_cargo {
            self.tests.phase = Phase::Failed;
            self.tests.phase_message = Some(
                "no workspace is open: open a .NET solution or a folder with a Cargo workspace"
                    .into(),
            );
            self.tests.dirty = true;
            self.refresh_tests_window(cx);
            return None;
        }
        let target = self.tests.shared.state.lock().map_or(0, |s| s.discoveries) + 1;
        if matches!(self.tests.phase, Phase::Building | Phase::Discovering) {
            return Some(target);
        }
        let stale = !self.tests.fresh || self.tests.generation != self.generation;
        if !stale && rebuild != Some(true) {
            return None;
        }
        self.tests.timings.discover_started = Some(Instant::now());
        self.tests.phase_message = None;
        let build = rebuild.unwrap_or(stale);
        if build && !self.builds.is_building() {
            let system = match (has_dotnet, has_cargo) {
                (true, true) => BuildSystemChoice::All,
                (true, false) => BuildSystemChoice::Msbuild,
                _ => BuildSystemChoice::Cargo,
            };
            self.builds.cargo_tests_next = has_cargo;
            let (started, _) = self.apply_build(
                BuildRequest::Start {
                    kind: BuildKind::Build,
                    project: None,
                    configuration: None,
                    platform: None,
                    system: Some(system),
                    wait: Some(false),
                },
                window,
                cx,
            );
            match started {
                Ok(_) => {
                    self.tests.phase = Phase::Building;
                    self.tests.build_ticket = self.builds.current.as_ref().map(|b| b.ticket);
                    self.tests.dirty = true;
                    self.refresh_tests_window(cx);
                    return Some(target);
                }
                Err(e) => {
                    self.builds.cargo_tests_next = false;
                    self.output.update(cx, |o, cx| {
                        o.append(
                            OutputSource::Tests,
                            &format!("The build did not start: {e}\n"),
                            cx,
                        )
                    });
                }
            }
        }
        self.discover_now(window, cx);
        Some(target)
    }

    /// Discover the outputs as they are: the host's containers and the Cargo targets.
    fn discover_now(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.tests.phase = Phase::Discovering;
        self.tests.build_ticket = None;
        self.tests.generation = self.generation;
        self.tests.dirty = true;
        self.output.update(cx, |o, cx| {
            o.append(
                OutputSource::Tests,
                "========== Starting test discovery ==========\n",
                cx,
            )
        });
        if self.solution.is_some() {
            self.tests.host_pending = true;
            let params = host::TestDiscoverParams {
                projects: None,
                configuration: Some(self.builds.configuration.clone()),
                run_settings: self.tests.run_settings.clone(),
                vstest_console_path: self.tests.vstest_console.clone(),
            };
            let (_, reply) = self.session.request::<host::TestDiscover>(params);
            cx.spawn_in(window, async move |this, cx| {
                let reply = reply.await;
                let _ = this.update_in(cx, |shell, window, cx| {
                    match reply.map(|r| r.result) {
                        Ok(Ok(found)) => {
                            shell.tests.host_discovery = Some(found.run_id);
                            if found.generation >= shell.generation {
                                let projects =
                                    found.containers.iter().map(project_of_container).collect();
                                shell.tests.replace_projects(false, projects);
                            }
                            shell.replay_early(found.run_id, window, cx);
                        }
                        Ok(Err(e)) => {
                            shell.tests.host_pending = false;
                            shell.tests.replace_projects(false, Vec::new());
                            shell.tests.phase_message =
                                Some(format!("eludite-host could not discover: {e:?}"));
                            shell.tests.set_discovery_done();
                        }
                        Err(_) => {
                            shell.tests.host_pending = false;
                            shell.tests.set_discovery_done();
                        }
                    }
                    shell.after_tests_change(cx);
                });
            })
            .detach();
        } else {
            self.tests.replace_projects(false, Vec::new());
        }
        let cargo = self.cargo_workspace().cloned();
        if let Some(ws) = cargo {
            let ticket = self.tests.next_listing;
            self.tests.next_listing += 1;
            self.tests.cargo_listing = Some(ticket);
            let mut projects = Vec::new();
            let mut jobs = Vec::new();
            for p in &ws.members {
                let targets = cargo_tests::test_targets(p);
                if targets.is_empty() {
                    continue;
                }
                projects.push(ProjectNode {
                    key: p.manifest_path.to_string_lossy().into_owned(),
                    name: p.name.clone(),
                    path: p.manifest_path.to_string_lossy().into_owned(),
                    protocol: Proto::Cargo,
                    target_framework: None,
                    runtime: None,
                    program: None,
                    state: NodeState::Discovering,
                    message: None,
                    cargo: Some((p.name.clone(), targets.clone())),
                });
                jobs.push(PackageJob {
                    name: p.name.clone(),
                    manifest: p.manifest_path.clone(),
                    targets: targets.into_iter().map(|t| (t, Vec::new())).collect(),
                });
            }
            self.tests.replace_projects(true, projects);
            let setup = CargoSetup {
                program: self.builds.cargo_program.clone(),
                manifest: ws.manifest.clone(),
                root: ws.root.clone(),
            };
            self.tests.cargo_list = Some(cargo_tests::list(
                ticket,
                setup,
                jobs,
                self.cargo_test_events.clone(),
            ));
        } else {
            self.tests.replace_projects(true, Vec::new());
        }
        self.tests.set_discovery_done();
        self.after_tests_change(cx);
    }

    /// The build the Test Explorer asked for ended (every build's end reaches here, through F5's hook): its discovery
    /// or run goes on. Any other build makes the kept discovery stale.
    pub(super) fn test_build_done(
        &mut self,
        ticket: u64,
        outcome: &PrelaunchBuild,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.tests.build_ticket != Some(ticket) {
            self.tests.fresh = false;
            // A debugged run waiting for its build.
            if let Some(run) = self
                .tests
                .runs
                .iter()
                .find(|r| r.state == RunState::Building && r.debug.is_some())
                .map(|r| r.id)
                && self.tests.pending_debug_build == Some(ticket)
            {
                self.tests.pending_debug_build = None;
                match outcome {
                    PrelaunchBuild::Succeeded => self.debug_after_build(run, window, cx),
                    PrelaunchBuild::Failed { errors, .. } => self.end_run(
                        run,
                        RunState::Failed,
                        Some(format!("the build failed ({errors} errors)")),
                        cx,
                    ),
                    PrelaunchBuild::Ended(why) => self.end_run(
                        run,
                        RunState::Failed,
                        Some(format!("the build ended ({why})")),
                        cx,
                    ),
                }
            }
            return;
        }
        self.tests.build_ticket = None;
        if let PrelaunchBuild::Failed { errors, .. } = outcome {
            self.tests.phase_message = Some(format!("the build failed ({errors} errors)"));
        }
        if let PrelaunchBuild::Ended(why) = outcome
            && why == "canceled"
        {
            self.tests.phase = Phase::Failed;
            self.tests.phase_message = Some("the build was canceled".into());
            self.tests.dirty = true;
            if let Some(run) = self.tests.waiting.take() {
                self.end_run(
                    run,
                    RunState::Canceled,
                    Some("the build was canceled".into()),
                    cx,
                );
            }
            self.refresh_tests_window(cx);
            return;
        }
        self.discover_now(window, cx);
    }

    /// `eludite/test/update` from the host.
    pub(super) fn on_test_update(
        &mut self,
        update: TestUpdate,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Computed for an older solution: not shown (CLAUDE.md invariant 12).
        if update.generation < self.generation {
            return;
        }
        let received = Instant::now();
        let is_discovery = self.tests.host_discovery == Some(update.run_id);
        let run = self
            .tests
            .runs
            .iter()
            .find(|r| r.host_run == Some(update.run_id))
            .map(|r| r.id);
        if !is_discovery && run.is_none() && update.kind != TestUpdateKind::Output {
            // Its reply is still on the way: keep it (a few thousand at most).
            if self.tests.early.len() < 10_000 {
                self.tests.early.push(update);
            }
            return;
        }
        let container = update.container.clone().unwrap_or_default();
        match update.kind {
            TestUpdateKind::Output => {
                if let Some(text) = &update.text {
                    self.output
                        .update(cx, |o, cx| o.append(OutputSource::Tests, text, cx));
                }
            }
            TestUpdateKind::Discovered if is_discovery => {
                for item in update.tests.iter().flatten() {
                    self.tests.add_test(dotnet_node(&container, item));
                }
            }
            TestUpdateKind::Results => {
                let Some(run) = run else { return };
                let dir = self
                    .tests
                    .project(&container)
                    .and_then(|p| Path::new(&p.path).parent().map(Path::to_path_buf));
                let results: Vec<(String, ResultData, Option<&TestResultItem>)> = update
                    .results
                    .iter()
                    .flatten()
                    .map(|r| {
                        let data = ResultData {
                            outcome: outcome_of(r.outcome),
                            duration_ms: r.duration_ms,
                            message: r.message.clone(),
                            stack_trace: r.stack_trace.clone(),
                            output: r.output.clone(),
                            failure: r
                                .stack_trace
                                .as_deref()
                                .and_then(|s| first_frame_in(s, dir.as_deref())),
                        };
                        (format!("{container}|{}", r.id), data, Some(r))
                    })
                    .collect();
                for (id, data, item) in results {
                    // A test discovery did not list (a data row expanded at run time) joins the tree.
                    if self.tests.test(&id).is_none()
                        && let Some(r) = item
                    {
                        let fqn = r
                            .fully_qualified_name
                            .clone()
                            .unwrap_or_else(|| r.id.clone());
                        let display = r.display_name.clone().unwrap_or_else(|| fqn.clone());
                        let (ns_cls, method) = fqn.rsplit_once('.').unwrap_or(("", &fqn));
                        let (ns, cls) = ns_cls.rsplit_once('.').unwrap_or(("", ns_cls));
                        self.tests.add_test(dotnet_node(
                            &container,
                            &TestItem {
                                id: r.id.clone(),
                                display_name: display,
                                fully_qualified_name: fqn.clone(),
                                namespace: Some(ns.to_owned()).filter(|n| !n.is_empty()),
                                class_name: Some(cls.to_owned()).filter(|c| !c.is_empty()),
                                method: Some(method.to_owned()),
                                source: None,
                                line: None,
                                traits: vec![],
                            },
                        ));
                        if let Some(rec) = self.tests.run_mut(run)
                            && !rec.tests.contains(&id)
                        {
                            rec.tests.push(id.clone());
                        }
                    }
                    self.record_result(run, &id, data, received);
                }
            }
            TestUpdateKind::ContainerFinished if is_discovery => {
                let count = self
                    .tests
                    .tests
                    .iter()
                    .filter(|t| t.project == container)
                    .count();
                if let Some(p) = self.tests.projects.iter_mut().find(|p| p.key == container) {
                    p.state = if update.state == Some(TestState::Completed) {
                        NodeState::Ready
                    } else {
                        NodeState::Failed
                    };
                    if update.message.is_some() {
                        p.message = update.message.clone();
                    }
                    if p.state == NodeState::Failed && count == 0 && p.message.is_none() {
                        p.message = Some("discovery failed".into());
                    }
                }
                self.tests.dirty = true;
            }
            TestUpdateKind::ContainerFinished => {
                if let (Some(run), Some(message)) = (run, update.message.clone())
                    && update.state == Some(TestState::Failed)
                {
                    let name = self
                        .tests
                        .project(&container)
                        .map_or(container.clone(), |p| p.name.clone());
                    self.output.update(cx, |o, cx| {
                        o.append(OutputSource::Tests, &format!("{name}: {message}\n"), cx)
                    });
                    if let Some(r) = self.tests.run_mut(run) {
                        r.message.get_or_insert(format!("{name}: {message}"));
                    }
                }
            }
            TestUpdateKind::Finished if is_discovery => {
                self.tests.host_pending = false;
                self.tests.host_discovery = None;
                for p in self
                    .tests
                    .projects
                    .iter_mut()
                    .filter(|p| p.protocol != Proto::Cargo)
                {
                    if p.state == NodeState::Discovering {
                        p.state = if update.state == Some(TestState::Canceled) {
                            NodeState::Failed
                        } else {
                            NodeState::Ready
                        };
                    }
                }
                self.tests.set_discovery_done();
            }
            TestUpdateKind::Finished => {
                if let Some(run) = run {
                    if let Some(r) = self.tests.run_mut(run) {
                        r.host_pending = false;
                        if update.state == Some(TestState::Canceled) {
                            r.canceled = true;
                        }
                        if update.state == Some(TestState::Failed)
                            && let Some(m) = &update.message
                        {
                            r.message.get_or_insert(m.clone());
                        }
                    }
                    self.maybe_finish_run(run, cx);
                }
            }
            TestUpdateKind::Launch => {
                if let (Some(run), Some(launch)) = (run, update.launch.clone()) {
                    self.debug_test_launch(run, &container, launch, window, cx);
                }
            }
            TestUpdateKind::Attach => {
                if let (Some(run), Some(pid)) = (run, update.process_id) {
                    self.debug_test_attach(run, pid, window, cx);
                }
            }
            TestUpdateKind::Discovered => {}
        }
        if matches!(self.tests.phase, Phase::Ready | Phase::Failed)
            && let Some(run) = self.tests.waiting.take()
        {
            self.dispatch_run(run, window, cx);
        }
        self.after_tests_change(cx);
    }

    /// Apply the updates of `host_run` that came before its reply, in order.
    fn replay_early(&mut self, host_run: u64, window: &mut Window, cx: &mut Context<Self>) {
        let (mut mine, rest): (Vec<TestUpdate>, Vec<TestUpdate>) =
            std::mem::take(&mut self.tests.early)
                .into_iter()
                .partition(|u| u.run_id == host_run);
        self.tests.early = rest;
        mine.sort_by_key(|u| u.seq);
        for u in mine {
            self.on_test_update(u, window, cx);
        }
    }

    /// `eludite/test/status` after a host restart: replay what is going, end what the host does not know.
    pub(super) fn on_test_status(
        &mut self,
        status: TestStatusResult,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let running: HashSet<u64> = status.running.iter().map(|r| r.run_id).collect();
        for r in &status.running {
            if r.generation < self.generation {
                continue;
            }
            if self.tests.host_discovery == Some(r.run_id) {
                for t in &r.tests {
                    self.tests.add_test(dotnet_node(&t.container, &t.test));
                }
            }
            if let Some(run) = self
                .tests
                .runs
                .iter()
                .find(|x| x.host_run == Some(r.run_id))
                .map(|x| x.id)
            {
                for res in &r.results {
                    let container = res.container.clone().unwrap_or_default();
                    let id = format!("{container}|{}", res.id);
                    let data = ResultData {
                        outcome: outcome_of(res.outcome),
                        duration_ms: res.duration_ms,
                        message: res.message.clone(),
                        stack_trace: res.stack_trace.clone(),
                        output: res.output.clone(),
                        failure: None,
                    };
                    self.record_result(run, &id, data, Instant::now());
                }
            }
        }
        // A discovery or run the restarted host never had ended with the old host.
        if let Some(d) = self.tests.host_discovery
            && !running.contains(&d)
        {
            self.tests.host_discovery = None;
            self.tests.host_pending = false;
            self.tests.phase_message = Some("eludite-host restarted during the discovery".into());
            self.tests.set_discovery_done();
        }
        let lost: Vec<u64> = self
            .tests
            .runs
            .iter()
            .filter(|r| r.host_pending && r.host_run.is_some_and(|h| !running.contains(&h)))
            .map(|r| r.id)
            .collect();
        for run in lost {
            if let Some(r) = self.tests.run_mut(run) {
                r.host_pending = false;
                r.message = Some("eludite-host restarted during the run".into());
            }
            self.maybe_finish_run(run, cx);
        }
        let _ = window;
        self.after_tests_change(cx);
    }

    /// A new solution generation: the .NET tests are forgotten; a run of them ends.
    pub(super) fn tests_new_generation(&mut self, cx: &mut Context<Self>) {
        self.tests.replace_projects(false, Vec::new());
        self.tests.fresh = false;
        self.tests.generation = self.generation;
        if self.tests.host_pending {
            self.tests.host_pending = false;
            self.tests.host_discovery = None;
            self.tests.set_discovery_done();
        }
        let going: Vec<u64> = self
            .tests
            .runs
            .iter()
            .filter(|r| r.host_pending)
            .map(|r| r.id)
            .collect();
        for run in going {
            if let Some(r) = self.tests.run_mut(run) {
                r.host_pending = false;
                r.canceled = true;
            }
            self.maybe_finish_run(run, cx);
        }
        self.after_tests_change(cx);
    }

    /// The Cargo test threads' events.
    pub(super) fn on_cargo_test_event(
        &mut self,
        event: CargoTestEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let received = Instant::now();
        match event {
            CargoTestEvent::Output(text) => {
                self.output
                    .update(cx, |o, cx| o.append(OutputSource::Tests, &text, cx));
            }
            CargoTestEvent::Listed {
                ticket,
                package,
                target,
                tests,
                error,
            } => {
                if self.tests.cargo_listing != Some(ticket) {
                    return;
                }
                let key = package.to_string_lossy().into_owned();
                for (name, source, line) in tests {
                    let mut group: Vec<String> =
                        target.prefix().map(str::to_owned).into_iter().collect();
                    let parts: Vec<&str> = name.split("::").collect();
                    group.extend(
                        parts[..parts.len().saturating_sub(1)]
                            .iter()
                            .map(|s| (*s).to_owned()),
                    );
                    let full_name = match target.prefix() {
                        Some(p) => format!("{p}::{name}"),
                        None => name.clone(),
                    };
                    self.tests.add_test(TestNode {
                        id: format!("{key}|{}|{name}", target.key()),
                        runner_id: format!("{}|{name}", target.key()),
                        project: key.clone(),
                        name: parts.last().map_or(name.clone(), |s| (*s).to_owned()),
                        full_name,
                        group,
                        source: source.map(|s| s.to_string_lossy().into_owned()),
                        line,
                        traits: Vec::new(),
                        result: None,
                        cargo: Some((target.clone(), name.clone())),
                    });
                }
                if let Some(e) = error
                    && let Some(p) = self.tests.projects.iter_mut().find(|p| p.key == key)
                {
                    p.message = Some(e);
                    p.state = NodeState::Failed;
                }
            }
            CargoTestEvent::ListingDone { ticket } => {
                if self.tests.cargo_listing != Some(ticket) {
                    return;
                }
                self.tests.cargo_listing = None;
                self.tests.cargo_list = None;
                for p in self
                    .tests
                    .projects
                    .iter_mut()
                    .filter(|p| p.protocol == Proto::Cargo)
                {
                    if p.state == NodeState::Discovering {
                        p.state = NodeState::Ready;
                    }
                }
                self.tests.set_discovery_done();
            }
            CargoTestEvent::Results {
                run,
                package,
                target,
                results,
            } => {
                let key = package.to_string_lossy().into_owned();
                let root = self.cargo_workspace().map(|w| w.root.clone());
                for r in results {
                    let id = format!("{key}|{}|{}", target.key(), r.name);
                    let failure = r.location.as_ref().map(|(file, line)| {
                        let p = Path::new(file);
                        let abs = if p.is_absolute() {
                            p.to_path_buf()
                        } else {
                            root.as_deref().map_or(p.to_path_buf(), |r| r.join(p))
                        };
                        (abs, *line)
                    });
                    let data = ResultData {
                        outcome: r.outcome,
                        duration_ms: r.duration_ms,
                        message: r.message.clone(),
                        stack_trace: r.stack_trace.clone(),
                        output: r.output.clone(),
                        failure,
                    };
                    self.record_result(run, &id, data, received);
                }
            }
            CargoTestEvent::TargetDone {
                run,
                package,
                target,
                error,
            } => {
                if let Some(e) = error {
                    let key = package.to_string_lossy().into_owned();
                    let name = self
                        .tests
                        .project(&key)
                        .map_or(key.clone(), |p| p.name.clone());
                    self.output.update(cx, |o, cx| {
                        o.append(
                            OutputSource::Tests,
                            &format!("{name} ({}): {e}\n", target.key()),
                            cx,
                        )
                    });
                    if let Some(r) = self.tests.run_mut(run) {
                        r.message.get_or_insert(format!("{name}: {e}"));
                    }
                }
            }
            CargoTestEvent::RunDone { run, canceled } => {
                if let Some(r) = self.tests.run_mut(run) {
                    r.cargo_pending = false;
                    r.canceled |= canceled;
                }
                self.tests.cargo = None;
                self.maybe_finish_run(run, cx);
            }
        }
        if matches!(self.tests.phase, Phase::Ready | Phase::Failed)
            && let Some(run) = self.tests.waiting.take()
        {
            self.dispatch_run(run, window, cx);
        }
        self.after_tests_change(cx);
    }

    /// A test's result in `run`: the run's record, the tree, the window.
    fn record_result(&mut self, run: u64, id: &str, data: ResultData, received: Instant) {
        if self.tests.timings.first_result_received.is_none() {
            self.tests.timings.first_result_received = Some(received);
        }
        if let Some(r) = self.tests.run_mut(run) {
            if !r.tests.iter().any(|t| t == id) && data.outcome != Outcome::Running {
                r.tests.push(id.to_owned());
            }
            // A running state never replaces an outcome already reported.
            let keep = r.results.get(id).is_some_and(|old| {
                old.outcome != Outcome::Running && data.outcome == Outcome::Running
            });
            if !keep {
                r.results.insert(id.to_owned(), data.clone());
            }
        }
        if let Some(&i) = self.tests.index.get(id) {
            self.tests.tests[i].result = Some(data);
            self.tests.dirty = true;
        }
    }

    /// Resolve a selection to test ids (the tree must be ready); `this` is the run being resolved, which the last run
    /// of Run Failed Tests and Repeat Last Run is not.
    fn resolve(
        &self,
        s: &Selection,
        this: u64,
        cx: &Context<Self>,
    ) -> Result<Vec<String>, CommandError> {
        let root = self.workspace_root();
        let in_projects: Option<HashSet<String>> = match &s.project {
            Some(p) => {
                let keys = self.tests.projects_named(p, root.as_deref());
                if keys.is_empty() {
                    return Err(CommandError::InvalidInput(format!(
                        "`{p}` is not a test project or Cargo package of the workspace (projects: {})",
                        self.tests
                            .projects
                            .iter()
                            .map(|p| p.name.as_str())
                            .collect::<Vec<_>>()
                            .join(", ")
                    )));
                }
                Some(keys.into_iter().collect())
            }
            None => None,
        };
        let scoped = |t: &&TestNode| in_projects.as_ref().is_none_or(|k| k.contains(&t.project));
        let mut ids: Vec<String> = if let Some(list) = &s.ids {
            for id in list {
                if self.tests.test(id).is_none() {
                    return Err(CommandError::InvalidInput(format!(
                        "no discovered test has the id `{id}` (eludite.test.discover lists them)"
                    )));
                }
            }
            list.clone()
        } else if s.failed_only || s.repeat_last {
            let last = self
                .tests
                .runs
                .iter()
                .rev()
                .find(|r| r.id != this && r.debug.is_none())
                .ok_or_else(|| {
                    CommandError::Failed("no test has run yet: there is no last run".into())
                })?;
            last.tests
                .iter()
                .filter(|id| {
                    !s.failed_only
                        || last
                            .results
                            .get(*id)
                            .is_some_and(|r| r.outcome == Outcome::Failed)
                })
                .cloned()
                .collect()
        } else if s.selection {
            let picked = self.tests_window.read(cx).selected_tests();
            if picked.is_empty() {
                self.test_at_caret(cx).into_iter().collect()
            } else {
                picked
            }
        } else {
            self.tests
                .tests
                .iter()
                .filter(scoped)
                .map(|t| t.id.clone())
                .collect()
        };
        if let Some(f) = &s.filter {
            ids.retain(|id| self.tests.test(id).is_some_and(|t| TestRuns::matches(t, f)));
        }
        ids.retain(|id| self.tests.test(id).is_some_and(|t| scoped(&t)));
        Ok(ids)
    }

    /// The test whose declaration is nearest above the active editor's caret.
    fn test_at_caret(&self, cx: &Context<Self>) -> Option<String> {
        let active = self.controller.active_document()?;
        let view = self.views.borrow().get(&active).cloned()?;
        let line = view.read(cx).editor().primary_head().row + 1;
        let path = super::documents::normalize_path(Path::new(&active));
        self.tests
            .tests
            .iter()
            .filter(|t| {
                t.source
                    .as_deref()
                    .is_some_and(|s| super::documents::normalize_path(Path::new(s)) == path)
                    && t.line.is_some_and(|l| l <= line + 1)
            })
            .max_by_key(|t| t.line)
            .map(|t| t.id.clone())
    }

    /// Start a run (discovering first when the tree is out of date). Returns its id.
    pub(super) fn test_run(
        &mut self,
        selection: Selection,
        debug: Option<DebugRun>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<u64, CommandError> {
        if let Some(cur) = self.tests.current {
            return Err(CommandError::Failed(format!(
                "test run {cur} is going; cancel it (eludite.test.cancel) or wait for it"
            )));
        }
        if (selection.failed_only || selection.repeat_last) && self.tests.last_run().is_none() {
            return Err(CommandError::Failed(
                "no test has run yet: there is no last run".into(),
            ));
        }
        // Ids are checked at once while the tree is current.
        if self.tests.fresh
            && self.tests.generation == self.generation
            && let Some(unknown) = selection
                .ids
                .iter()
                .flatten()
                .find(|id| self.tests.test(id).is_none())
        {
            return Err(CommandError::InvalidInput(format!(
                "no discovered test has the id `{unknown}` (eludite.test.discover lists them)"
            )));
        }
        let id = self.tests.next_run;
        self.tests.next_run += 1;
        let record = RunRecord {
            id,
            state: RunState::Discovering,
            selection,
            tests: Vec::new(),
            results: HashMap::new(),
            started: Instant::now(),
            elapsed: None,
            host_run: None,
            host_pending: false,
            cargo_pending: false,
            canceled: false,
            message: None,
            debug,
        };
        self.tests.runs.push_back(record);
        while self.tests.runs.len() > KEPT_RUNS {
            self.tests.runs.pop_front();
        }
        self.tests.current = Some(id);
        self.tests.running.store(true, Ordering::SeqCst);
        self.tests.timings.run_started = Some(Instant::now());
        self.tests.timings.first_result_received = None;
        self.tests.timings.first_result_shown = None;
        self.status.set(TESTS_SLOT, "Tests: discovering\u{2026}");
        let stale = !self.tests.fresh || self.tests.generation != self.generation;
        if stale || matches!(self.tests.phase, Phase::Building | Phase::Discovering) {
            self.tests.waiting = Some(id);
            if self.test_discover(None, window, cx).is_none() {
                // The kept discovery answers after all.
                self.tests.waiting = None;
                self.dispatch_run(id, window, cx);
            } else if self.tests.phase == Phase::Building
                && let Some(r) = self.tests.run_mut(id)
            {
                r.state = RunState::Building;
            }
        } else {
            self.dispatch_run(id, window, cx);
        }
        self.after_tests_change(cx);
        Ok(id)
    }

    /// The tree is ready: resolve the run's tests and start the host's and cargo's parts.
    fn dispatch_run(&mut self, run: u64, window: &mut Window, cx: &mut Context<Self>) {
        let Some(selection) = self.tests.run(run).map(|r| r.selection.clone()) else {
            return;
        };
        let ids = match self.resolve(&selection, run, cx) {
            Ok(ids) => ids,
            Err(e) => return self.end_run(run, RunState::Failed, Some(e.to_string()), cx),
        };
        if ids.is_empty() {
            let why = self
                .tests
                .phase_message
                .clone()
                .map_or("no test matches".to_owned(), |m| {
                    format!("no test matches ({m})")
                });
            return self.end_run(run, RunState::Passed, Some(why), cx);
        }
        let debug = self.tests.run(run).and_then(|r| r.debug.clone());
        if let Some(r) = self.tests.run_mut(run) {
            r.tests = ids.clone();
            r.state = RunState::Running;
            r.started = Instant::now();
        }
        // Results of the tests this run covers start over.
        for id in &ids {
            if let Some(&i) = self.tests.index.get(id) {
                self.tests.tests[i].result = None;
            }
        }
        self.tests.dirty = true;
        self.output.update(cx, |o, cx| {
            o.append(
                OutputSource::Tests,
                &format!(
                    "========== Starting test run: {} tests ==========\n",
                    ids.len()
                ),
                cx,
            )
        });
        if debug.is_some() {
            return self.debug_dispatch(run, ids, window, cx);
        }
        // .NET: the containers and their tests (all of a container's tests: the container alone).
        let mut containers: BTreeMap<String, Vec<String>> = BTreeMap::new();
        let mut cargo: BTreeMap<String, BTreeMap<String, (TestTarget, Vec<String>)>> =
            BTreeMap::new();
        for id in &ids {
            let Some(t) = self.tests.test(id) else {
                continue;
            };
            match &t.cargo {
                Some((target, name)) => {
                    cargo
                        .entry(t.project.clone())
                        .or_default()
                        .entry(target.key())
                        .or_insert_with(|| (target.clone(), Vec::new()))
                        .1
                        .push(name.clone());
                }
                None => containers
                    .entry(t.project.clone())
                    .or_default()
                    .push(t.runner_id.clone()),
            }
        }
        if !containers.is_empty() {
            let list = containers
                .into_iter()
                .map(|(id, tests)| {
                    let all =
                        self.tests.tests.iter().filter(|t| t.project == id).count() == tests.len();
                    host::TestRunContainer {
                        id,
                        tests: (!all).then_some(tests),
                    }
                })
                .collect();
            self.start_host_run(run, list, false, window, cx);
        }
        if !cargo.is_empty()
            && let Some(ws) = self.cargo_workspace().cloned()
        {
            let jobs = cargo
                .into_iter()
                .map(|(manifest, targets)| PackageJob {
                    name: self
                        .tests
                        .project(&manifest)
                        .and_then(|p| p.cargo.as_ref().map(|(n, _)| n.clone()))
                        .unwrap_or_default(),
                    manifest: PathBuf::from(manifest),
                    targets: targets.into_values().collect(),
                })
                .collect();
            let setup = CargoSetup {
                program: self.builds.cargo_program.clone(),
                manifest: ws.manifest.clone(),
                root: ws.root.clone(),
            };
            if let Some(r) = self.tests.run_mut(run) {
                r.cargo_pending = true;
            }
            let release = self.builds.configuration.eq_ignore_ascii_case("release");
            self.tests.cargo = Some(cargo_tests::run(
                run,
                setup,
                jobs,
                self.tests.parallel,
                release,
                self.cargo_test_events.clone(),
            ));
        }
        self.maybe_finish_run(run, cx);
    }

    /// `eludite/test/run` for the host part of a run.
    fn start_host_run(
        &mut self,
        run: u64,
        containers: Vec<host::TestRunContainer>,
        debug: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(r) = self.tests.run_mut(run) {
            r.host_pending = true;
        }
        let params = host::TestRunParams {
            containers: Some(containers),
            debug: debug.then_some(true),
            parallel: Some(self.tests.parallel),
            configuration: Some(self.builds.configuration.clone()),
            run_settings: self.tests.run_settings.clone(),
            vstest_console_path: self.tests.vstest_console.clone(),
        };
        let (_, reply) = self.session.request::<host::TestRun>(params);
        cx.spawn_in(window, async move |this, cx| {
            let reply = reply.await;
            let _ = this.update_in(cx, |shell, window, cx| {
                match reply.map(|r| r.result) {
                    Ok(Ok(started)) => {
                        if let Some(r) = shell.tests.run_mut(run) {
                            r.host_run = Some(started.run_id);
                        }
                        shell.replay_early(started.run_id, window, cx);
                    }
                    other => {
                        let why = match other {
                            Ok(Err(super::session::RequestError::Failed(m))) => m,
                            Ok(Err(e)) => format!("{e:?}"),
                            _ => "eludite-host did not answer".into(),
                        };
                        if let Some(r) = shell.tests.run_mut(run) {
                            r.host_pending = false;
                            r.message = Some(format!("the .NET tests did not run: {why}"));
                        }
                        if shell
                            .tests
                            .run(run)
                            .and_then(|r| r.debug.as_ref())
                            .is_some()
                        {
                            shell.tests.shared.update(|s| {
                                s.debug.insert(run, Err(why.clone()));
                            });
                        }
                        shell.maybe_finish_run(run, cx);
                    }
                }
                shell.after_tests_change(cx);
            });
        })
        .detach();
    }

    /// A run ends when its parts did.
    fn maybe_finish_run(&mut self, run: u64, cx: &mut Context<Self>) {
        let Some(r) = self.tests.run(run) else { return };
        if r.state.done() || r.host_pending || r.cargo_pending || r.state != RunState::Running {
            return;
        }
        if r.debug
            .as_ref()
            .is_some_and(|d| d.session.is_some() || matches!(d.kind, DebugKind::Cargo { .. }))
            && !r.canceled
            && self.debug_session_live(run)
        {
            return;
        }
        let state = if r.canceled {
            RunState::Canceled
        } else if r.results.values().any(|x| x.outcome == Outcome::Failed) {
            RunState::Failed
        } else {
            RunState::Passed
        };
        self.end_run(run, state, None, cx);
    }

    fn end_run(
        &mut self,
        run: u64,
        state: RunState,
        message: Option<String>,
        cx: &mut Context<Self>,
    ) {
        if let Some(r) = self.tests.run_mut(run) {
            if r.state.done() {
                return;
            }
            r.state = state;
            r.elapsed = Some(r.started.elapsed());
            if message.is_some() {
                r.message = message;
            }
            // Tests still running when it ended did not finish.
            for x in r.results.values_mut() {
                if x.outcome == Outcome::Running {
                    x.outcome = Outcome::NotRun;
                }
            }
        }
        if self.tests.current == Some(run) {
            self.tests.current = None;
            self.tests.running.store(false, Ordering::SeqCst);
        }
        if self.tests.waiting == Some(run) {
            self.tests.waiting = None;
        }
        // Tests still marked running in the tree ran in this run and did not finish.
        let ids: Vec<String> = self
            .tests
            .run(run)
            .map(|r| r.tests.clone())
            .unwrap_or_default();
        for id in ids {
            if let Some(&i) = self.tests.index.get(&id)
                && let Some(res) = self.tests.tests[i].result.as_mut()
                && res.outcome == Outcome::Running
            {
                res.outcome = Outcome::NotRun;
            }
        }
        self.tests.timings.run_done = Some(Instant::now());
        if let Some(r) = self.tests.run(run) {
            let s = r.summary();
            let text = if r.state == RunState::Canceled {
                format!("{} (canceled)", summary_text(&s))
            } else {
                summary_text(&s)
            };
            self.output.update(cx, |o, cx| {
                o.append(
                    OutputSource::Tests,
                    &format!("========== {text} ==========\n"),
                    cx,
                )
            });
            if let Some(m) = &r.message {
                self.output.update(cx, |o, cx| {
                    o.append(OutputSource::Tests, &format!("{m}\n"), cx)
                });
            }
            self.status.set(TESTS_SLOT, text);
        }
        let why = self
            .tests
            .run(run)
            .and_then(|r| r.message.clone())
            .unwrap_or_else(|| "the run ended before a debugging session started".into());
        self.tests.shared.update(|s| {
            s.finished_runs.insert(run);
            s.debug.entry(run).or_insert(Err(why));
        });
        self.tests.dirty = true;
        self.update_error_list(cx);
        cx.notify();
    }

    /// `eludite.test.cancel`.
    pub(super) fn test_cancel(&mut self, run: Option<u64>, cx: &mut Context<Self>) -> CancelOutput {
        let going = self.tests.current;
        let Some(id) = going.filter(|g| run.is_none_or(|r| r == *g)) else {
            return CancelOutput {
                canceled: false,
                run: None,
            };
        };
        if let Some(c) = &self.tests.cargo {
            c.cancel();
        }
        let (host_run, waiting_build, debug_session) = match self.tests.run_mut(id) {
            Some(r) => {
                r.canceled = true;
                (
                    r.host_run.filter(|_| r.host_pending),
                    r.state == RunState::Building,
                    r.debug.as_ref().and_then(|d| d.session),
                )
            }
            None => (None, false, None),
        };
        if let Some(h) = host_run {
            let (_, _reply) = self
                .session
                .request::<host::TestCancel>(host::TestCancelParams { run_id: Some(h) });
        }
        if waiting_build && self.builds.is_building() {
            let _ = self.apply_build_cancel(cx);
        }
        if let Some(session) = debug_session {
            let _ = self
                .commands
                .invoke(eludite_commands::debug::STOP, json!({ "session": session }));
        }
        if self.tests.waiting == Some(id) || waiting_build {
            self.tests.waiting = None;
            self.end_run(id, RunState::Canceled, None, cx);
        } else {
            self.maybe_finish_run(id, cx);
        }
        CancelOutput {
            canceled: true,
            run: Some(id),
        }
    }

    fn apply_build_cancel(&mut self, cx: &mut Context<Self>) -> bool {
        if let Some(c) = &self.builds.cargo {
            c.cancel();
        }
        self.session.build_cancel();
        cx.notify();
        true
    }

    /// The Error List's rows for the last run's failed tests.
    pub(super) fn test_error_rows(&self) -> Vec<(PathBuf, u32, String, String, Option<String>)> {
        let Some(run) = self.tests.last_run().filter(|r| r.state.done()) else {
            return Vec::new();
        };
        let mut rows = Vec::new();
        for id in &run.tests {
            let Some(res) = run.results.get(id).filter(|r| r.outcome == Outcome::Failed) else {
                continue;
            };
            let Some(t) = self.tests.test(id) else {
                continue;
            };
            let (path, line) = match (&res.failure, &t.source, t.line) {
                (Some((p, l)), _, _) => (p.clone(), *l),
                (None, Some(s), Some(l)) => (PathBuf::from(s), l),
                _ => (
                    PathBuf::from(
                        self.tests
                            .project(&t.project)
                            .map_or("", |p| p.path.as_str()),
                    ),
                    1,
                ),
            };
            let first = res
                .message
                .as_deref()
                .and_then(|m| m.lines().next())
                .unwrap_or("failed");
            let project = self.tests.project(&t.project).map(|p| p.name.clone());
            rows.push((
                path,
                line,
                t.full_name.clone(),
                format!("Test failed: {}: {first}", t.full_name),
                project,
            ));
        }
        rows
    }

    /// After the model changed: the window gets the tree again (when it changed) and the window repaints.
    pub(super) fn after_tests_change(&mut self, cx: &mut Context<Self>) {
        self.refresh_tests_window(cx);
        cx.notify();
    }

    pub(super) fn refresh_tests_window(&mut self, cx: &mut Context<Self>) {
        if !self.tests.dirty {
            return;
        }
        self.tests.dirty = false;
        let started = Instant::now();
        let data = super::tests_window::TreeData {
            projects: self.tests.projects.clone(),
            tests: self.tests.tests.clone(),
            phase: self.tests.phase,
            message: self.tests.phase_message.clone(),
            summary: self
                .tests
                .last_run()
                .filter(|r| r.state.done())
                .map(|r| (summary_text(&r.summary()), r.summary())),
            running: self.tests.current.is_some(),
        };
        self.tests_window.update(cx, |w, cx| w.set_data(data, cx));
        self.tests.timings.tree_built = Some(started.elapsed());
        if self.tests.timings.first_result_received.is_some()
            && self.tests.timings.first_result_shown.is_none()
        {
            self.tests.timings.first_result_shown = Some(Instant::now());
        }
    }

    // ---- Answers -------------------------------------------------------------------------------------------------

    fn discover_output(&self, request: &TestRequest) -> DiscoverOutput {
        let (project, max_items, cursor) = match request {
            TestRequest::Discover {
                project,
                max_items,
                cursor,
                ..
            } => (project.clone(), *max_items, cursor.clone()),
            _ => (None, cmds::DEFAULT_PAGE, None),
        };
        let root = self.workspace_root();
        let keys: Option<HashSet<String>> = project.as_deref().map(|p| {
            self.tests
                .projects_named(p, root.as_deref())
                .into_iter()
                .collect()
        });
        let projects: Vec<&ProjectNode> = self
            .tests
            .projects
            .iter()
            .filter(|p| keys.as_ref().is_none_or(|k| k.contains(&p.key)))
            .collect();
        let in_tree: Vec<&TestNode> = self.tree_order(keys.as_ref());
        let start: usize = cursor.and_then(|c| c.parse().ok()).unwrap_or(0);
        let page: Vec<DiscoverTest> = in_tree
            .iter()
            .skip(start)
            .take(max_items)
            .map(|t| DiscoverTest {
                id: t.id.clone(),
                name: t.name.clone(),
                full_name: t.full_name.clone(),
                project: self
                    .tests
                    .project(&t.project)
                    .map_or(String::new(), |p| p.name.clone()),
                group: t.group.clone(),
                source: t.source.clone(),
                line: t.line,
                traits: t.traits.iter().map(|(n, v)| format!("{n}={v}")).collect(),
                outcome: t.outcome(),
            })
            .collect();
        let next = start + page.len();
        let state = match self.tests.phase {
            Phase::Building => "building",
            Phase::Discovering => "discovering",
            Phase::Failed => "failed",
            Phase::Ready | Phase::Idle => "ready",
        };
        let mut message = self.tests.phase_message.clone();
        if project.is_some()
            && keys.as_ref().is_some_and(HashSet::is_empty)
            && self.tests.phase == Phase::Ready
        {
            message = Some(format!(
                "`{}` is not a test project or Cargo package of the workspace",
                project.as_deref().unwrap_or_default()
            ));
        }
        DiscoverOutput {
            state: state.into(),
            generation: self.generation,
            counts: DiscoverCounts {
                projects: projects.len() as u32,
                tests: in_tree.len() as u32,
            },
            projects: projects
                .iter()
                .map(|p| DiscoverProject {
                    name: p.name.clone(),
                    path: p.path.clone(),
                    protocol: p.protocol.as_str().into(),
                    target_framework: p.target_framework.clone(),
                    tests: self
                        .tests
                        .tests
                        .iter()
                        .filter(|t| t.project == p.key)
                        .count() as u32,
                    state: match p.state {
                        NodeState::Discovering => "discovering",
                        NodeState::Ready => "ready",
                        NodeState::Failed => "failed",
                    }
                    .into(),
                    message: p.message.clone(),
                })
                .collect(),
            truncated: next < in_tree.len(),
            next_cursor: (next < in_tree.len()).then(|| next.to_string()),
            total: in_tree.len() as u32,
            tests: page,
            message,
        }
    }

    /// Tests in tree order: by project, then group, then name.
    fn tree_order(&self, keys: Option<&HashSet<String>>) -> Vec<&TestNode> {
        let rank: HashMap<&str, usize> = self
            .tests
            .projects
            .iter()
            .enumerate()
            .map(|(i, p)| (p.key.as_str(), i))
            .collect();
        let mut list: Vec<&TestNode> = self
            .tests
            .tests
            .iter()
            .filter(|t| keys.is_none_or(|k| k.contains(&t.project)))
            .collect();
        list.sort_by(|a, b| {
            rank.get(a.project.as_str())
                .cmp(&rank.get(b.project.as_str()))
                .then_with(|| a.group.cmp(&b.group))
                .then_with(|| a.name.cmp(&b.name))
        });
        list
    }

    fn result_row(
        &self,
        id: &str,
        r: &ResultData,
        max_chars: usize,
        with_output: bool,
    ) -> ResultRow {
        let t = self.tests.test(id);
        let mut truncated = false;
        let mut cut = |s: &Option<String>| {
            s.as_deref().map(|s| {
                let (c, cut) = cmds::cut(s, max_chars);
                truncated |= cut;
                c
            })
        };
        let message = cut(&r.message);
        let stack_trace = cut(&r.stack_trace);
        let output = if with_output { cut(&r.output) } else { None };
        let (source, line) = match (&r.failure, t) {
            (Some((p, l)), _) => (Some(p.to_string_lossy().into_owned()), Some(*l)),
            (None, Some(t)) => (t.source.clone(), t.line),
            _ => (None, None),
        };
        ResultRow {
            id: id.to_owned(),
            name: t.map_or(id.to_owned(), |t| {
                if t.name.contains('(') {
                    format!(
                        "{}{}",
                        t.full_name
                            .rsplit_once('.')
                            .map_or(String::new(), |(p, _)| format!("{p}.")),
                        t.name
                    )
                } else {
                    t.full_name.clone()
                }
            }),
            project: t
                .and_then(|t| self.tests.project(&t.project))
                .map_or(String::new(), |p| p.name.clone()),
            outcome: r.outcome,
            duration_ms: r.duration_ms,
            message,
            stack_trace,
            output,
            source,
            line,
            truncated,
        }
    }

    fn run_output(&self, id: u64, timed_out: bool) -> RunOutput {
        let Some(r) = self.tests.run(id) else {
            return RunOutput {
                run: id,
                state: "failed".into(),
                tests: 0,
                summary: None,
                failed: Vec::new(),
                timed_out,
                message: Some("the run is no longer kept".into()),
            };
        };
        let failed = r
            .tests
            .iter()
            .filter_map(|t| {
                r.results
                    .get(t)
                    .filter(|x| x.outcome == Outcome::Failed)
                    .map(|x| (t, x))
            })
            .take(cmds::RUN_FAILED_LISTED)
            .map(|(t, x)| self.result_row(t, x, cmds::RUN_MESSAGE_CHARS, false))
            .collect();
        RunOutput {
            run: id,
            state: r.state.as_str().into(),
            tests: r.tests.len() as u32,
            summary: (r.state.done() || timed_out || !r.results.is_empty()).then(|| r.summary()),
            failed,
            timed_out: timed_out && !r.state.done(),
            message: r.message.clone(),
        }
    }

    fn results_output(&self, request: &TestRequest) -> Outcome2 {
        let TestRequest::Results {
            run,
            outcome,
            ids,
            max_output_chars,
            max_items,
            cursor,
        } = request
        else {
            unreachable!("a results request")
        };
        let r = match run {
            Some(id) => self.tests.run(*id).ok_or_else(|| {
                CommandError::InvalidInput(format!(
                    "no run {id} is kept (the last {KEPT_RUNS} are)"
                ))
            })?,
            None => self
                .tests
                .last_run()
                .ok_or_else(|| CommandError::Failed("no test has run yet".into()))?,
        };
        let rows: Vec<ResultRow> = r
            .tests
            .iter()
            .filter(|t| ids.as_ref().is_none_or(|l| l.contains(t)))
            .map(|t| {
                let data = r.results.get(t).cloned().unwrap_or(ResultData {
                    outcome: Outcome::NotRun,
                    duration_ms: None,
                    message: None,
                    stack_trace: None,
                    output: None,
                    failure: None,
                });
                (t, data)
            })
            .filter(|(_, d)| outcome.is_none_or(|o| d.outcome == o))
            .map(|(t, d)| self.result_row(t, &d, *max_output_chars, true))
            .collect();
        let start: usize = cursor.as_deref().and_then(|c| c.parse().ok()).unwrap_or(0);
        let total = rows.len();
        let page: Vec<ResultRow> = rows.into_iter().skip(start).take(*max_items).collect();
        let next = start + page.len();
        Ok(TestOutput::Results(Box::new(ResultsOutput {
            run: r.id,
            state: r.state.as_str().into(),
            summary: r.summary(),
            results: page,
            total: total as u32,
            next_cursor: (next < total).then(|| next.to_string()),
            truncated: next < total,
        })))
    }

    fn debug_output(&self, run: u64, summary: Option<Value>, cx: &Context<Self>) -> DebugOutput {
        let r = self.tests.run(run);
        let d = r.and_then(|r| r.debug.as_ref());
        let summary = summary.unwrap_or_else(|| {
            let mode = match r.map(|r| r.state) {
                Some(RunState::Building) => "building",
                Some(RunState::Discovering) => "launching",
                Some(s) if s.done() => "design",
                _ => self.debug.model.mode.as_str(),
            };
            let mut v = json!({ "mode": mode });
            if let Some(s) = d.and_then(|d| d.session) {
                v["session"] = json!(s);
            }
            if let Some(m) = r.and_then(|r| r.message.clone()) {
                v["message"] = json!(m);
            }
            v
        });
        let _ = cx;
        DebugOutput {
            run,
            project: d.map_or(String::new(), |d| d.project.clone()),
            tests: r.map(|r| r.tests.clone()).unwrap_or_default(),
            breakpoint: d.and_then(|d| d.breakpoint.clone()),
            summary,
        }
    }

    // ---- Debug Test ----------------------------------------------------------------------------------------------

    /// `eludite.test.debug`: a run under the debugger (its tests resolved once the tree is ready).
    pub(super) fn test_debug(
        &mut self,
        selection: Selection,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<u64, CommandError> {
        let plan = DebugRun {
            kind: DebugKind::Host,
            project: String::new(),
            breakpoint: None,
            session: None,
            attaching: None,
            output_from: 0,
        };
        self.test_run(selection, Some(plan), window, cx)
    }

    /// The debugged run's tests are known: they must be of one project; build, then start.
    fn debug_dispatch(
        &mut self,
        run: u64,
        ids: Vec<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let mut projects: Vec<String> = ids
            .iter()
            .filter_map(|id| self.tests.test(id).map(|t| t.project.clone()))
            .collect();
        projects.dedup();
        let all: HashSet<&String> = projects.iter().collect();
        // Debug All Tests without a project: the first test project.
        let selection = self
            .tests
            .run(run)
            .map(|r| r.selection.clone())
            .unwrap_or_default();
        let ids = if all.len() > 1 {
            let plain =
                selection.ids.is_none() && selection.filter.is_none() && !selection.selection;
            if !plain {
                return self.end_run(
                    run,
                    RunState::Failed,
                    Some(
                        "the tests to debug must be of one project; name it with `project`".into(),
                    ),
                    cx,
                );
            }
            let first = projects[0].clone();
            ids.into_iter()
                .filter(|id| self.tests.test(id).is_some_and(|t| t.project == first))
                .collect()
        } else {
            ids
        };
        let Some(first) = ids.first().and_then(|id| self.tests.test(id)).cloned() else {
            return self.end_run(run, RunState::Failed, Some("no test to debug".into()), cx);
        };
        let Some(project) = self.tests.project(&first.project).cloned() else {
            return;
        };
        // The temporary function breakpoint on the first test.
        let breakpoint = match &first.cargo {
            Some((target, name)) => {
                let krate = match target.prefix() {
                    Some(p) => p.replace('-', "_"),
                    None => project
                        .cargo
                        .as_ref()
                        .map_or(String::new(), |(n, _)| n.replace('-', "_")),
                };
                format!("{krate}::{name}")
            }
            None => first.full_name.clone(),
        };
        let kind = match &first.cargo {
            Some((target, _)) => DebugKind::Cargo {
                manifest: project.path.clone(),
                target: match target.kind {
                    eludite_workspace::cargo::TargetKind::Lib
                    | eludite_workspace::cargo::TargetKind::ProcMacro => None,
                    _ => Some(target.name.clone()),
                },
                names: ids
                    .iter()
                    .filter_map(|id| {
                        self.tests
                            .test(id)
                            .and_then(|t| t.cargo.as_ref().map(|(_, n)| n.clone()))
                    })
                    .collect(),
            },
            None => DebugKind::Host,
        };
        if let Some(r) = self.tests.run_mut(run) {
            r.tests = ids.clone();
            if let Some(d) = r.debug.as_mut() {
                d.kind = kind.clone();
                d.project = project.name.clone();
                d.breakpoint = Some(breakpoint.clone());
            }
        }
        let set = self.invoke(
            eludite_commands::debug::TOGGLE_BREAKPOINT,
            json!({ "function": breakpoint, "action": "set", "remove_after": true }),
            window,
            cx,
        );
        if let Err(e) = set {
            self.output.update(cx, |o, cx| {
                o.append(
                    OutputSource::Tests,
                    &format!("The breakpoint on the first test was not set: {e}\n"),
                    cx,
                )
            });
        }
        match kind {
            DebugKind::Cargo { .. } => self.debug_after_build(run, window, cx),
            DebugKind::Host => {
                // Build the project first (the build gate of F5), then ask the host.
                if !self.builds.build_before_run {
                    return self.debug_after_build(run, window, cx);
                }
                let started = self.invoke(
                    eludite_commands::build::PROJECT,
                    json!({ "project": project.path, "wait": false }),
                    window,
                    cx,
                );
                match started {
                    Ok(_) => {
                        self.tests.pending_debug_build =
                            self.builds.current.as_ref().map(|b| b.ticket);
                        if let Some(r) = self.tests.run_mut(run) {
                            r.state = RunState::Building;
                        }
                    }
                    Err(e) => self.end_run(
                        run,
                        RunState::Failed,
                        Some(format!("the build did not start: {e}")),
                        cx,
                    ),
                }
            }
        }
    }

    /// The debugged run's build succeeded (or none was needed): start it.
    fn debug_after_build(&mut self, run: u64, window: &mut Window, cx: &mut Context<Self>) {
        let Some(r) = self.tests.run(run).cloned() else {
            return;
        };
        let Some(d) = r.debug.clone() else { return };
        if let Some(rec) = self.tests.run_mut(run) {
            rec.state = RunState::Running;
        }
        match d.kind {
            DebugKind::Cargo {
                manifest,
                target,
                names,
            } => {
                let mut args = names.clone();
                args.extend(["--exact".into(), "--test-threads=1".into()]);
                let mut start = json!({
                    "project": manifest,
                    "test": true,
                    "args": args,
                });
                if let Some(t) = target {
                    start["target"] = json!(t);
                }
                let from = self.output.read(cx).pane(OutputSource::Debug).len();
                let out = self.invoke(eludite_commands::debug::START, start, window, cx);
                if let Some(rec) = self.tests.run_mut(run)
                    && let Some(dd) = rec.debug.as_mut()
                {
                    dd.output_from = from;
                }
                let failed = out.as_ref().err().map(|e| e.to_string());
                self.debug_started(run, out, cx);
                if let Some(why) = failed {
                    self.end_run(run, RunState::Failed, Some(why), cx);
                }
            }
            DebugKind::Host => {
                let first = r.tests.first().and_then(|id| self.tests.test(id)).cloned();
                let Some(first) = first else { return };
                let all = self
                    .tests
                    .tests
                    .iter()
                    .filter(|t| t.project == first.project)
                    .count()
                    == r.tests.len();
                let tests: Vec<String> = r
                    .tests
                    .iter()
                    .filter_map(|id| self.tests.test(id).map(|t| t.runner_id.clone()))
                    .collect();
                self.start_host_run(
                    run,
                    vec![host::TestRunContainer {
                        id: first.project.clone(),
                        tests: (!all).then_some(tests),
                    }],
                    true,
                    window,
                    cx,
                );
            }
        }
    }

    /// The host's `launch` update: start the test application under its adapter (the debugger's start seam).
    fn debug_test_launch(
        &mut self,
        run: u64,
        container: &str,
        launch: TestLaunch,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(project) = self.tests.project(container).cloned() else {
            return;
        };
        let kind = match launch.runtime {
            Some(TestRuntime::Mono | TestRuntime::Netfx) => {
                eludite_dap::launch::FrameworkKind::NetFramework
            }
            _ => eludite_dap::launch::FrameworkKind::CoreClr,
        };
        self.debug.test_launch = Some(eludite_dap::launch::LaunchConfig {
            project: PathBuf::from(&project.path),
            program: PathBuf::from(&launch.program),
            kind,
            args: launch.args.clone(),
            cwd: PathBuf::from(&launch.cwd),
            env: launch.env.clone(),
            profile: None,
        });
        let out = self.invoke(
            eludite_commands::debug::START,
            json!({ "project": project.path, "build": false }),
            window,
            cx,
        );
        self.debug.test_launch = None;
        self.debug_started(run, out, cx);
    }

    /// The host's `attach` update: attach the adapter to the paused testhost, then answer the host.
    fn debug_test_attach(
        &mut self,
        run: u64,
        pid: u32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let out = self.invoke(
            eludite_commands::debug::ATTACH,
            json!({ "pid": pid, "adapter": "coreclr" }),
            window,
            cx,
        );
        if let Some(r) = self.tests.run_mut(run)
            && let Some(d) = r.debug.as_mut()
        {
            d.attaching = Some(pid);
        }
        let failed = out.as_ref().err().map(|e| e.to_string());
        self.debug_started(run, out, cx);
        if let Some(why) = failed {
            self.answer_attached(run, false, Some(why), cx);
        }
    }

    fn answer_attached(
        &mut self,
        run: u64,
        attached: bool,
        message: Option<String>,
        cx: &mut Context<Self>,
    ) {
        let (host_run, pid) = match self.tests.run_mut(run) {
            Some(r) => (
                r.host_run,
                r.debug.as_mut().and_then(|d| d.attaching.take()),
            ),
            None => (None, None),
        };
        if let (Some(h), Some(pid)) = (host_run, pid) {
            let (_, _reply) =
                self.session
                    .request::<host::TestAttached>(host::TestAttachedParams {
                        run_id: h,
                        process_id: pid,
                        attached,
                        message,
                    });
        }
        cx.notify();
    }

    /// A debugged run's start answered (`eludite.debug.start` or `attach`'s stop summary, with its `session`).
    fn debug_started(
        &mut self,
        run: u64,
        out: Result<Value, CommandError>,
        cx: &mut Context<Self>,
    ) {
        match out {
            Ok(summary) => {
                let session = summary["session"].as_u64().map(|s| s as u32);
                if let Some(r) = self.tests.run_mut(run)
                    && let Some(d) = r.debug.as_mut()
                {
                    d.session = session;
                }
                if let Some(s) = session {
                    self.tests.shared.update(|st| {
                        st.debug.insert(run, Ok(s));
                    });
                }
                self.watch_debug_run(run, cx);
            }
            Err(e) => {
                let why = e.to_string();
                self.tests.shared.update(|st| {
                    st.debug.insert(run, Err(why.clone()));
                });
                if let Some(r) = self.tests.run_mut(run) {
                    r.message = Some(format!("the debugging session did not start: {why}"));
                }
            }
        }
    }

    /// Whether the debugged run's session is still live.
    fn debug_session_live(&self, run: u64) -> bool {
        let Some(session) = self
            .tests
            .run(run)
            .and_then(|r| r.debug.as_ref())
            .and_then(|d| d.session)
        else {
            return false;
        };
        self.debug.live_ids().contains(&session)
    }

    /// Follow a debugged run: answer the attach once the session runs, and end a Cargo run (reading its results from
    /// the Debug pane) once the session ended.
    fn watch_debug_run(&mut self, run: u64, cx: &mut Context<Self>) {
        // A ticker thread, not an executor timer: the follow-up must run in real time (the session's adapter and the
        // host are real processes) wherever the UI runs.
        let (tick, mut ticks) = futures::channel::mpsc::unbounded::<()>();
        let spawned = std::thread::Builder::new()
            .name("eludite-test-debug-watch".into())
            .spawn(move || {
                while tick.unbounded_send(()).is_ok() {
                    std::thread::sleep(Duration::from_millis(50));
                }
            });
        if spawned.is_err() {
            return;
        }
        cx.spawn(async move |this, cx| {
            use futures::StreamExt as _;
            while ticks.next().await.is_some() {
                let Ok(done) = this.update(cx, |shell, cx| shell.debug_run_tick(run, cx)) else {
                    break;
                };
                if done {
                    break;
                }
            }
        })
        .detach();
    }

    /// One look at a debugged run; true when there is nothing more to follow.
    fn debug_run_tick(&mut self, run: u64, cx: &mut Context<Self>) -> bool {
        let Some(r) = self.tests.run(run).cloned() else {
            return true;
        };
        if r.state.done() {
            return true;
        }
        let Some(d) = r.debug.clone() else {
            return true;
        };
        let live = self.debug_session_live(run);
        if d.attaching.is_some() {
            let mode = self.debug.model.mode;
            use super::debug::state::Mode;
            if matches!(mode, Mode::Running | Mode::Break) || !live {
                self.answer_attached(
                    run,
                    live,
                    (!live).then(|| "the debugging session ended".into()),
                    cx,
                );
            }
        }
        if live {
            return false;
        }
        // The session ended: a Cargo run's results are libtest's lines in the Debug pane.
        if matches!(d.kind, DebugKind::Cargo { .. }) {
            let lines: Vec<String> = {
                let o = self.output.read(cx);
                let pane = o.pane(OutputSource::Debug);
                (d.output_from..pane.len())
                    .filter_map(|i| pane.line(i).map(str::to_owned))
                    .collect()
            };
            let mut parser = cargo_tests::Libtest::default();
            let now = Instant::now();
            let mut results = Vec::new();
            for l in &lines {
                results.extend(parser.stdout(l, now));
                results.extend(parser.stderr(l));
            }
            results.extend(parser.finish_output(now));
            let manifest = match &d.kind {
                DebugKind::Cargo { manifest, .. } => manifest.clone(),
                DebugKind::Host => String::new(),
            };
            let target_key = r
                .tests
                .first()
                .and_then(|id| self.tests.test(id))
                .and_then(|t| t.cargo.as_ref().map(|(t, _)| t.key()))
                .unwrap_or_else(|| "lib".into());
            for res in results {
                let id = format!("{manifest}|{target_key}|{}", res.name);
                let data = ResultData {
                    outcome: res.outcome,
                    duration_ms: res.duration_ms,
                    message: res.message,
                    stack_trace: res.stack_trace,
                    output: res.output,
                    failure: None,
                };
                self.record_result(run, &id, data, now);
            }
            if let Some(rec) = self.tests.run_mut(run)
                && let Some(dd) = rec.debug.as_mut()
            {
                dd.session = None;
            }
            self.maybe_finish_run(run, cx);
        } else if let Some(rec) = self.tests.run_mut(run) {
            if let Some(dd) = rec.debug.as_mut() {
                dd.session = None;
            }
            // The host's run ends on its own (its finished update); a session that never ran it ends it here.
            if rec.host_run.is_none() || !rec.host_pending {
                self.maybe_finish_run(run, cx);
            }
        }
        self.after_tests_change(cx);
        true
    }
}

#[cfg(test)]
mod unit_tests {
    use super::*;

    #[test]
    fn the_test_menu_waits_for_a_run() {
        assert!(menu_enabled(cmds::RUN, false));
        assert!(!menu_enabled(cmds::RUN, true));
        assert!(!menu_enabled(cmds::DEBUG, true));
        assert!(menu_enabled(cmds::EXPLORER, true));
        assert!(menu_enabled(cmds::CANCEL, true));
    }

    #[test]
    fn stack_frames_pick_the_project_frame() {
        let stack = "   at Xunit.Assert.Equal(Int32 a) in /_/src/Assert.cs:line 10\n   at Corpus.C.M() in /s/Corpus/C.cs:line 25\n   at System.Reflection.Invoke()";
        assert_eq!(stack_frames(stack).len(), 2);
        assert_eq!(
            first_frame_in(stack, Some(Path::new("/s/Corpus"))),
            Some((PathBuf::from("/s/Corpus/C.cs"), 25))
        );
        assert_eq!(
            first_frame_in(stack, None),
            Some((PathBuf::from("/_/src/Assert.cs"), 10))
        );
        assert_eq!(first_frame_in("no frames", None), None);
    }

    #[test]
    fn filters_names_and_summaries() {
        let t = TestNode {
            id: "p|u".into(),
            runner_id: "u".into(),
            project: "p".into(),
            name: "AddsPairs(a: 1)".into(),
            full_name: "Corpus.CalculatorTests.AddsPairs".into(),
            group: vec!["Corpus".into(), "CalculatorTests".into()],
            source: None,
            line: None,
            traits: vec![("Category".into(), "Math".into())],
            result: None,
            cargo: None,
        };
        assert!(TestRuns::matches(&t, "calculatortests.adds"));
        assert!(TestRuns::matches(&t, "category = math"));
        assert!(!TestRuns::matches(&t, "Category=Slow"));
        assert!(!TestRuns::matches(&t, "Subtracts"));
        let node = dotnet_node(
            "c",
            &TestItem {
                id: "u1".into(),
                display_name: "Corpus.CalculatorTests.AddsPairs(a: 1, b: 1, sum: 2)".into(),
                fully_qualified_name: "Corpus.CalculatorTests.AddsPairs".into(),
                namespace: Some("Corpus".into()),
                class_name: Some("CalculatorTests".into()),
                method: Some("AddsPairs".into()),
                source: None,
                line: None,
                traits: vec![],
            },
        );
        assert_eq!(node.name, "AddsPairs(a: 1, b: 1, sum: 2)");
        assert_eq!(node.id, "c|u1");
        assert_eq!(node.group, ["Corpus", "CalculatorTests"]);
        let plain = dotnet_node(
            "c",
            &TestItem {
                id: "u2".into(),
                display_name: "ParsesNumbers".into(),
                fully_qualified_name: "Corpus.ParserTests.ParsesNumbers".into(),
                namespace: Some("Corpus".into()),
                class_name: Some("ParserTests".into()),
                method: Some("ParsesNumbers".into()),
                source: None,
                line: None,
                traits: vec![],
            },
        );
        assert_eq!(plain.name, "ParsesNumbers");
        assert_eq!(
            summary_text(&RunSummary {
                total: 15,
                passed: 12,
                failed: 1,
                skipped: 2,
                not_run: 0,
                duration_ms: Some(3200.)
            }),
            "Tests: 12 passed, 1 failed, 2 skipped (3.2 s)"
        );
    }
}
