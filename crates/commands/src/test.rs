//! The Test Explorer's commands (brief 0035): `eludite.test.explorer` (the window), `eludite.test.discover`,
//! `eludite.test.run`, `eludite.test.debug`, `eludite.test.results` and `eludite.test.cancel`.
//!
//! The schemas are the files in `protocol/schemas/test-*.json` (checked in first, CLAUDE.md invariant 4). This module
//! parses input into a typed [`TestRequest`] and serializes the typed outputs; the shell implements [`TestCommands`].
//! `discover` and `results` read; `run`, `debug` and `cancel` execute (they start or stop processes). From the UI
//! thread the commands never wait; from another thread (an agent) `discover`, `run` and `debug` wait for their
//! `wait_ms` (defaults 60,000, 30,000 and 30,000).

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{CommandError, CommandId, CommandRegistry, CommandSpec, PermissionClass};

pub const EXPLORER: &str = "eludite.test.explorer";
pub const DISCOVER: &str = "eludite.test.discover";
pub const RUN: &str = "eludite.test.run";
pub const DEBUG: &str = "eludite.test.debug";
pub const RESULTS: &str = "eludite.test.results";
pub const CANCEL: &str = "eludite.test.cancel";

pub const ALL: [&str; 6] = [EXPLORER, DISCOVER, RUN, DEBUG, RESULTS, CANCEL];

/// The longest wait any test command takes.
pub const MAX_WAIT_MS: u64 = 300_000;
/// An agent's default waits.
pub const AGENT_DISCOVER_WAIT_MS: u64 = 60_000;
pub const AGENT_RUN_WAIT_MS: u64 = 30_000;
/// Tests a discover page lists by default, and at most.
pub const DEFAULT_PAGE: usize = 500;
pub const MAX_PAGE: usize = 5_000;
/// Results a results page lists by default.
pub const DEFAULT_RESULTS_PAGE: usize = 100;
/// Each message, stack trace and output of `results` is cut here by default, and at most here.
pub const DEFAULT_OUTPUT_CHARS: usize = 4_000;
pub const MAX_OUTPUT_CHARS: usize = 100_000;
/// A run's answer: failed tests listed at most, and each message cut at.
pub const RUN_FAILED_LISTED: usize = 50;
pub const RUN_MESSAGE_CHARS: usize = 2_000;

/// (title, input schema, output schema, permission, agent visible)
fn schemas(
    id: &str,
) -> (
    &'static str,
    &'static str,
    &'static str,
    PermissionClass,
    bool,
) {
    use PermissionClass::*;
    match id {
        EXPLORER => (
            "Test: Test Explorer",
            include_str!("../../../protocol/schemas/test-explorer.input.json"),
            crate::view::TOOL_WINDOW_OUTPUT_SCHEMA,
            Read,
            false,
        ),
        DISCOVER => (
            "Test: Discover Tests",
            include_str!("../../../protocol/schemas/test-discover.input.json"),
            include_str!("../../../protocol/schemas/test-discover.output.json"),
            Read,
            true,
        ),
        RUN => (
            "Test: Run Tests",
            include_str!("../../../protocol/schemas/test-run.input.json"),
            include_str!("../../../protocol/schemas/test-run.output.json"),
            Execute,
            true,
        ),
        DEBUG => (
            "Test: Debug Tests",
            include_str!("../../../protocol/schemas/test-debug.input.json"),
            include_str!("../../../protocol/schemas/test-debug.output.json"),
            Execute,
            true,
        ),
        RESULTS => (
            "Test: Results",
            include_str!("../../../protocol/schemas/test-results.input.json"),
            include_str!("../../../protocol/schemas/test-results.output.json"),
            Read,
            true,
        ),
        CANCEL => (
            "Test: Cancel",
            include_str!("../../../protocol/schemas/test-cancel.input.json"),
            include_str!("../../../protocol/schemas/test-cancel.output.json"),
            Execute,
            true,
        ),
        other => unreachable!("not a test command: {other}"),
    }
}

/// A test's state, as the commands name it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    Passed,
    Failed,
    Skipped,
    NotRun,
    Running,
}

impl Outcome {
    pub fn as_str(self) -> &'static str {
        match self {
            Outcome::Passed => "passed",
            Outcome::Failed => "failed",
            Outcome::Skipped => "skipped",
            Outcome::NotRun => "not_run",
            Outcome::Running => "running",
        }
    }
}

/// Which tests a run or debug covers.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Selection {
    pub ids: Option<Vec<String>>,
    /// A substring of the fully qualified name, or a trait `Name=Value`.
    pub filter: Option<String>,
    pub project: Option<String>,
    /// Run Failed Tests.
    pub failed_only: bool,
    /// Repeat Last Run.
    pub repeat_last: bool,
    /// The Test Explorer's selected node, else the test at the caret.
    pub selection: bool,
}

/// A parsed, validated test command.
#[derive(Debug, Clone, PartialEq)]
pub enum TestRequest {
    /// Show the window; `filter` sets its search box.
    Explorer {
        filter: Option<String>,
    },
    Discover {
        project: Option<String>,
        rebuild: Option<bool>,
        wait_ms: Option<u64>,
        max_items: usize,
        cursor: Option<String>,
    },
    Run {
        selection: Selection,
        wait_ms: Option<u64>,
    },
    Debug {
        selection: Selection,
        wait_ms: Option<u64>,
        /// The stop summary's budget parameters, as given (`depth`, `max_variables`, ...), for `eludite.debug.wait`.
        budget: serde_json::Map<String, Value>,
    },
    Results {
        run: Option<u64>,
        outcome: Option<Outcome>,
        ids: Option<Vec<String>>,
        max_output_chars: usize,
        max_items: usize,
        cursor: Option<String>,
    },
    Cancel {
        run: Option<u64>,
    },
}

impl TestRequest {
    pub fn command(&self) -> &'static str {
        match self {
            TestRequest::Explorer { .. } => EXPLORER,
            TestRequest::Discover { .. } => DISCOVER,
            TestRequest::Run { .. } => RUN,
            TestRequest::Debug { .. } => DEBUG,
            TestRequest::Results { .. } => RESULTS,
            TestRequest::Cancel { .. } => CANCEL,
        }
    }

    /// How long the call waits from another thread when it does not say.
    pub fn agent_wait_ms(&self) -> Option<u64> {
        match self {
            TestRequest::Discover { wait_ms, .. } => {
                Some(wait_ms.unwrap_or(AGENT_DISCOVER_WAIT_MS))
            }
            TestRequest::Run { wait_ms, .. } | TestRequest::Debug { wait_ms, .. } => {
                Some(wait_ms.unwrap_or(AGENT_RUN_WAIT_MS))
            }
            _ => None,
        }
    }
}

/// `counts` of `test-discover.output.json`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DiscoverCounts {
    pub projects: u32,
    pub tests: u32,
}

/// A project of `test-discover.output.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DiscoverProject {
    pub name: String,
    pub path: String,
    /// `mtp`, `vstest` or `cargo`.
    pub protocol: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_framework: Option<String>,
    pub tests: u32,
    /// `discovering`, `ready` or `failed`.
    pub state: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

/// A test of `test-discover.output.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DiscoverTest {
    pub id: String,
    pub name: String,
    pub full_name: String,
    pub project: String,
    pub group: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub traits: Vec<String>,
    pub outcome: Outcome,
}

/// `test-discover.output.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DiscoverOutput {
    /// `building`, `discovering`, `ready` or `failed`.
    pub state: String,
    pub generation: u64,
    pub counts: DiscoverCounts,
    pub projects: Vec<DiscoverProject>,
    pub tests: Vec<DiscoverTest>,
    pub total: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
    pub truncated: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

/// `summary` of the run and results outputs.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunSummary {
    pub total: u32,
    pub passed: u32,
    pub failed: u32,
    pub skipped: u32,
    pub not_run: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<f64>,
}

/// A test of the run and results outputs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResultRow {
    pub id: String,
    pub name: String,
    pub project: String,
    pub outcome: Outcome,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stack_trace: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub truncated: bool,
}

/// `test-run.output.json`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunOutput {
    pub run: u64,
    /// `building`, `discovering`, `running`, `passed`, `failed` or `canceled`.
    pub state: String,
    pub tests: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<RunSummary>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub failed: Vec<ResultRow>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub timed_out: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

/// `test-debug.output.json`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DebugOutput {
    pub run: u64,
    pub project: String,
    pub tests: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub breakpoint: Option<String>,
    /// The stop summary (debug-stop-summary.output.json), or a smaller one while the session starts.
    pub summary: Value,
}

/// `test-results.output.json`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResultsOutput {
    pub run: u64,
    pub state: String,
    pub summary: RunSummary,
    pub results: Vec<ResultRow>,
    pub total: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
    pub truncated: bool,
}

/// `test-cancel.output.json`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CancelOutput {
    pub canceled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run: Option<u64>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum TestOutput {
    /// `eludite.test.explorer`: the window's place (view-tool-window.output.json).
    Explorer(Value),
    Discover(Box<DiscoverOutput>),
    Run(Box<RunOutput>),
    Debug(Box<DebugOutput>),
    Results(Box<ResultsOutput>),
    Cancel(CancelOutput),
}

impl TestOutput {
    pub fn to_json(&self) -> Value {
        match self {
            TestOutput::Explorer(v) => Ok(v.clone()),
            TestOutput::Discover(o) => serde_json::to_value(o),
            TestOutput::Run(o) => serde_json::to_value(o),
            TestOutput::Debug(o) => serde_json::to_value(o),
            TestOutput::Results(o) => serde_json::to_value(o),
            TestOutput::Cancel(o) => serde_json::to_value(o),
        }
        .expect("test outputs serialize")
    }
}

/// Whatever owns the Test Explorer (the shell). Called on the invoking thread.
pub trait TestCommands: Send + Sync {
    fn apply(&self, request: TestRequest) -> Result<TestOutput, CommandError>;
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct ExplorerIn {
    filter: Option<String>,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct DiscoverIn {
    project: Option<String>,
    rebuild: Option<bool>,
    wait_ms: Option<u64>,
    max_items: Option<u64>,
    cursor: Option<String>,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct RunIn {
    ids: Option<Vec<String>>,
    filter: Option<String>,
    project: Option<String>,
    failed_only: Option<bool>,
    repeat_last: Option<bool>,
    selection: Option<bool>,
    wait_ms: Option<u64>,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct DebugIn {
    ids: Option<Vec<String>>,
    filter: Option<String>,
    project: Option<String>,
    selection: Option<bool>,
    wait_ms: Option<u64>,
    depth: Option<u64>,
    max_variables: Option<u64>,
    max_value_chars: Option<u64>,
    max_frames: Option<u64>,
    max_output_lines: Option<u64>,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct ResultsIn {
    run: Option<u64>,
    outcome: Option<Outcome>,
    ids: Option<Vec<String>>,
    max_output_chars: Option<u64>,
    max_items: Option<u64>,
    cursor: Option<String>,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct CancelIn {
    run: Option<u64>,
}

fn input<T: for<'de> Deserialize<'de> + Default>(value: Value) -> Result<T, CommandError> {
    if value.is_null() {
        return Ok(T::default());
    }
    serde_json::from_value(value).map_err(|e| CommandError::InvalidInput(e.to_string()))
}

fn invalid(message: impl Into<String>) -> CommandError {
    CommandError::InvalidInput(message.into())
}

fn non_empty(name: &str, v: Option<String>) -> Result<Option<String>, CommandError> {
    match v {
        Some(s) if s.trim().is_empty() => Err(invalid(format!("`{name}` must not be empty"))),
        other => Ok(other),
    }
}

fn ids(v: Option<Vec<String>>) -> Result<Option<Vec<String>>, CommandError> {
    match v {
        Some(list) if list.is_empty() => Err(invalid("`ids` lists at least one test")),
        Some(list) if list.iter().any(|i| i.is_empty()) => {
            Err(invalid("`ids` must not hold an empty id"))
        }
        other => Ok(other),
    }
}

fn wait(v: Option<u64>) -> Result<Option<u64>, CommandError> {
    match v {
        Some(ms) if ms > MAX_WAIT_MS => Err(invalid(format!("`wait_ms` is at most {MAX_WAIT_MS}"))),
        other => Ok(other),
    }
}

fn bounded(
    name: &str,
    v: Option<u64>,
    default: usize,
    min: usize,
    max: usize,
) -> Result<usize, CommandError> {
    match v {
        None => Ok(default),
        Some(n) if (min as u64..=max as u64).contains(&n) => Ok(n as usize),
        Some(_) => Err(invalid(format!("`{name}` is {min} to {max}"))),
    }
}

/// Parse and validate the input of test command `id`.
pub fn parse(id: &str, value: Value) -> Result<TestRequest, CommandError> {
    Ok(match id {
        EXPLORER => {
            let i: ExplorerIn = input(value)?;
            TestRequest::Explorer { filter: i.filter }
        }
        DISCOVER => {
            let i: DiscoverIn = input(value)?;
            TestRequest::Discover {
                project: non_empty("project", i.project)?,
                rebuild: i.rebuild,
                wait_ms: wait(i.wait_ms)?,
                max_items: bounded("max_items", i.max_items, DEFAULT_PAGE, 1, MAX_PAGE)?,
                cursor: non_empty("cursor", i.cursor)?,
            }
        }
        RUN => {
            let i: RunIn = input(value)?;
            TestRequest::Run {
                selection: Selection {
                    ids: ids(i.ids)?,
                    filter: non_empty("filter", i.filter)?,
                    project: non_empty("project", i.project)?,
                    failed_only: i.failed_only.unwrap_or(false),
                    repeat_last: i.repeat_last.unwrap_or(false),
                    selection: i.selection.unwrap_or(false),
                },
                wait_ms: wait(i.wait_ms)?,
            }
        }
        DEBUG => {
            let i: DebugIn = input(value)?;
            let mut budget = serde_json::Map::new();
            for (name, v, min, max) in [
                ("depth", i.depth, 1, 5),
                ("max_variables", i.max_variables, 1, 500),
                ("max_value_chars", i.max_value_chars, 1, 10_000),
                ("max_frames", i.max_frames, 1, 200),
                ("max_output_lines", i.max_output_lines, 0, 1_000),
            ] {
                if let Some(n) = v {
                    bounded(name, Some(n), 0, min, max)?;
                    budget.insert(name.into(), Value::from(n));
                }
            }
            TestRequest::Debug {
                selection: Selection {
                    ids: ids(i.ids)?,
                    filter: non_empty("filter", i.filter)?,
                    project: non_empty("project", i.project)?,
                    selection: i.selection.unwrap_or(false),
                    ..Selection::default()
                },
                wait_ms: wait(i.wait_ms)?,
                budget,
            }
        }
        RESULTS => {
            let i: ResultsIn = input(value)?;
            TestRequest::Results {
                run: i.run,
                outcome: i.outcome,
                ids: ids(i.ids)?,
                max_output_chars: bounded(
                    "max_output_chars",
                    i.max_output_chars,
                    DEFAULT_OUTPUT_CHARS,
                    0,
                    MAX_OUTPUT_CHARS,
                )?,
                max_items: bounded("max_items", i.max_items, DEFAULT_RESULTS_PAGE, 1, MAX_PAGE)?,
                cursor: non_empty("cursor", i.cursor)?,
            }
        }
        CANCEL => {
            let i: CancelIn = input(value)?;
            TestRequest::Cancel { run: i.run }
        }
        other => return Err(CommandError::UnknownCommand(other.to_owned())),
    })
}

/// The public description of command `id` (one of [`ALL`]).
pub fn spec(id: &str) -> CommandSpec {
    let (title, input, output, permission, agent_visible) = schemas(id);
    CommandSpec {
        id: CommandId::new(id).expect("valid id"),
        title: title.into(),
        input_schema: serde_json::from_str(input).expect("protocol schemas are valid JSON"),
        output_schema: serde_json::from_str(output).expect("protocol schemas are valid JSON"),
        permission,
        agent_visible,
    }
}

/// Register the test commands, applying them to `target`.
pub fn register(registry: &CommandRegistry, target: Arc<dyn TestCommands>) {
    for id in ALL {
        let target = target.clone();
        registry.replace(spec(id), move |input| {
            let request = parse(id, input)?;
            target.apply(request).map(|out| out.to_json())
        });
    }
}

/// `text` cut at `max` characters (on a character boundary), and whether it was cut.
pub fn cut(text: &str, max: usize) -> (String, bool) {
    match text.char_indices().nth(max) {
        Some((at, _)) => (text[..at].to_owned(), true),
        None => (text.to_owned(), false),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn schema(text: &str) -> Value {
        serde_json::from_str(text).unwrap()
    }

    /// Every member of `value` is a property of `schema`, and every required one is there.
    fn fits(value: &Value, schema: &Value) {
        for r in schema["required"].as_array().into_iter().flatten() {
            assert!(value.get(r.as_str().unwrap()).is_some(), "missing {r}");
        }
        for k in value.as_object().unwrap().keys() {
            assert!(schema["properties"].get(k).is_some(), "unexpected {k}");
        }
    }

    #[test]
    fn parses_and_validates_every_command() {
        assert_eq!(
            parse(EXPLORER, json!({})).unwrap(),
            TestRequest::Explorer { filter: None }
        );
        assert_eq!(
            parse(EXPLORER, json!({"filter": "Outcome:Failed"})).unwrap(),
            TestRequest::Explorer {
                filter: Some("Outcome:Failed".into())
            }
        );
        assert!(parse(EXPLORER, json!({"x": 1})).is_err());
        assert_eq!(
            parse(DISCOVER, Value::Null).unwrap(),
            TestRequest::Discover {
                project: None,
                rebuild: None,
                wait_ms: None,
                max_items: DEFAULT_PAGE,
                cursor: None
            }
        );
        assert!(parse(DISCOVER, json!({"max_items": 0})).is_err());
        assert!(parse(DISCOVER, json!({"max_items": 5001})).is_err());
        assert!(parse(DISCOVER, json!({"wait_ms": 300_001})).is_err());
        assert!(parse(DISCOVER, json!({"project": " "})).is_err());
        let run = parse(
            RUN,
            json!({"filter": "Category=Math", "failed_only": true, "wait_ms": 1000}),
        )
        .unwrap();
        assert_eq!(
            run,
            TestRequest::Run {
                selection: Selection {
                    filter: Some("Category=Math".into()),
                    failed_only: true,
                    ..Selection::default()
                },
                wait_ms: Some(1000)
            }
        );
        assert_eq!(run.agent_wait_ms(), Some(1000));
        assert_eq!(
            parse(RUN, json!({})).unwrap().agent_wait_ms(),
            Some(AGENT_RUN_WAIT_MS)
        );
        assert!(parse(RUN, json!({"ids": []})).is_err());
        assert!(parse(RUN, json!({"ids": [""]})).is_err());
        assert!(parse(RUN, json!({"outcome": "failed"})).is_err());
        let debug = parse(
            DEBUG,
            json!({"ids": ["p|u1"], "depth": 2, "max_frames": 5, "wait_ms": 10}),
        )
        .unwrap();
        let TestRequest::Debug {
            budget, selection, ..
        } = &debug
        else {
            panic!("{debug:?}")
        };
        assert_eq!(selection.ids.as_deref(), Some(&["p|u1".to_owned()][..]));
        assert_eq!(
            Value::Object(budget.clone()),
            json!({"depth": 2, "max_frames": 5})
        );
        assert!(parse(DEBUG, json!({"depth": 6})).is_err());
        assert!(parse(DEBUG, json!({"failed_only": true})).is_err());
        assert_eq!(
            parse(RESULTS, json!({"outcome": "not_run"})).unwrap(),
            TestRequest::Results {
                run: None,
                outcome: Some(Outcome::NotRun),
                ids: None,
                max_output_chars: DEFAULT_OUTPUT_CHARS,
                max_items: DEFAULT_RESULTS_PAGE,
                cursor: None
            }
        );
        assert!(parse(RESULTS, json!({"max_output_chars": 100_001})).is_err());
        assert_eq!(
            parse(CANCEL, json!({"run": 3})).unwrap(),
            TestRequest::Cancel { run: Some(3) }
        );
        assert!(parse(CANCEL, json!({"run": 0, "x": 1})).is_err());
        assert!(parse("eludite.test.nope", json!({})).is_err());
    }

    #[test]
    fn specs_and_outputs_follow_the_schemas() {
        for id in ALL {
            let s = spec(id);
            assert_eq!(s.input_schema["title"], format!("{id} input"));
            assert_eq!(s.input_schema["additionalProperties"], false, "{id}");
        }
        assert_eq!(spec(DISCOVER).permission, PermissionClass::Read);
        assert_eq!(spec(RESULTS).permission, PermissionClass::Read);
        for id in [RUN, DEBUG, CANCEL] {
            assert_eq!(spec(id).permission, PermissionClass::Execute);
        }
        assert!(!spec(EXPLORER).agent_visible);
        assert!(spec(RUN).agent_visible);

        let row = ResultRow {
            id: "p|u1".into(),
            name: "N.C.M".into(),
            project: "Corpus.XunitV3".into(),
            outcome: Outcome::Failed,
            duration_ms: Some(2.0),
            message: Some("boom".into()),
            stack_trace: Some("at N.C.M()".into()),
            output: None,
            source: Some("/s/C.cs".into()),
            line: Some(25),
            truncated: true,
        };
        let run = TestOutput::Run(Box::new(RunOutput {
            run: 1,
            state: "failed".into(),
            tests: 2,
            summary: Some(RunSummary {
                total: 2,
                passed: 1,
                failed: 1,
                skipped: 0,
                not_run: 0,
                duration_ms: Some(10.0),
            }),
            failed: vec![row.clone()],
            timed_out: false,
            message: None,
        }))
        .to_json();
        let s = schema(schemas(RUN).2);
        fits(&run, &s);
        fits(&run["summary"], &s["properties"]["summary"]);
        fits(&run["failed"][0], &s["properties"]["failed"]["items"]);
        assert_eq!(run["failed"][0]["outcome"], "failed");
        let discover = TestOutput::Discover(Box::new(DiscoverOutput {
            state: "ready".into(),
            generation: 2,
            counts: DiscoverCounts {
                projects: 1,
                tests: 1,
            },
            projects: vec![DiscoverProject {
                name: "corpus-tests".into(),
                path: "/w/Cargo.toml".into(),
                protocol: "cargo".into(),
                target_framework: None,
                tests: 1,
                state: "ready".into(),
                message: None,
            }],
            tests: vec![DiscoverTest {
                id: "/w/Cargo.toml|lib|tests::adds".into(),
                name: "adds".into(),
                full_name: "tests::adds".into(),
                project: "corpus-tests".into(),
                group: vec!["tests".into()],
                source: None,
                line: None,
                traits: vec![],
                outcome: Outcome::NotRun,
            }],
            total: 1,
            next_cursor: None,
            truncated: false,
            message: None,
        }))
        .to_json();
        let s = schema(schemas(DISCOVER).2);
        fits(&discover, &s);
        fits(
            &discover["projects"][0],
            &s["properties"]["projects"]["items"],
        );
        fits(&discover["tests"][0], &s["properties"]["tests"]["items"]);
        assert_eq!(discover["tests"][0]["outcome"], "not_run");
        let results = TestOutput::Results(Box::new(ResultsOutput {
            run: 1,
            state: "passed".into(),
            summary: RunSummary::default(),
            results: vec![row],
            total: 1,
            next_cursor: Some("1".into()),
            truncated: true,
        }))
        .to_json();
        fits(&results, &schema(schemas(RESULTS).2));
        let debug = TestOutput::Debug(Box::new(DebugOutput {
            run: 1,
            project: "corpus-tests".into(),
            tests: vec!["x".into()],
            breakpoint: Some("corpus_tests::tests::adds".into()),
            summary: json!({"mode": "break"}),
        }))
        .to_json();
        fits(&debug, &schema(schemas(DEBUG).2));
        let cancel = TestOutput::Cancel(CancelOutput {
            canceled: true,
            run: Some(1),
        })
        .to_json();
        fits(&cancel, &schema(schemas(CANCEL).2));
        assert_eq!(cut("abcdé", 4), ("abcd".to_owned(), true));
        assert_eq!(cut("abc", 4), ("abc".to_owned(), false));
    }

    struct Echo;
    impl TestCommands for Echo {
        fn apply(&self, request: TestRequest) -> Result<TestOutput, CommandError> {
            match request {
                TestRequest::Cancel { run } => Ok(TestOutput::Cancel(CancelOutput {
                    canceled: run.is_some(),
                    run,
                })),
                other => Err(CommandError::Failed(format!("{other:?}"))),
            }
        }
    }

    #[test]
    fn registers_and_routes_to_the_target() {
        let r = CommandRegistry::new();
        register(&r, Arc::new(Echo));
        assert_eq!(
            r.invoke(CANCEL, json!({"run": 2})).unwrap(),
            json!({"canceled": true, "run": 2})
        );
        assert!(matches!(
            r.invoke(RUN, json!({"nope": 1})),
            Err(CommandError::InvalidInput(_))
        ));
        assert_eq!(
            r.lookup(DEBUG).unwrap().permission,
            PermissionClass::Execute
        );
    }
}
