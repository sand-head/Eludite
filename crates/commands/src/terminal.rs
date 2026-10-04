//! The integrated terminal's commands (brief 0041): `eludite.terminal.*`, the Terminal window's actions for the
//! person and agents alike: open (New Terminal, Split, Restart), list, send (typing), read, wait, resize, close (the
//! tab's close and Kill Terminal) and clear.
//!
//! The schemas are `protocol/schemas/terminal-*.json` (checked in first, CLAUDE.md invariant 4). This module parses
//! input into a typed [`TerminalRequest`], serializes the typed outputs, declares each command's class and registers
//! the escalation hooks that apply the solution policy's `terminal` object ([`crate::policy::TerminalPolicy`]). The
//! shell implements [`TerminalCommands`] over `eludite-terminal`, on the invoking thread (a wait blocks its caller,
//! never the UI thread).
//!
//! Classes: `list`, `read` and `wait` are read (always allowed for agents); `open`, `send`, `resize`, `clear` and
//! `close` are execute, and for agents `terminal.run` decides: `prompt` (default) asks at the first such call of an
//! agent session and \"Allow for this session\" grants the rest, `allow` runs them, `deny` refuses them; `close`
//! with `kill` is dangerous.

use std::collections::BTreeMap;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::policy::{PolicyView, TERMINAL_RUN_GRANT};
use crate::{
    CommandError, CommandId, CommandRegistry, CommandSpec, Escalation, EscalationHook,
    PermissionClass,
};

pub const OPEN: &str = "eludite.terminal.open";
pub const LIST: &str = "eludite.terminal.list";
pub const SEND: &str = "eludite.terminal.send";
pub const READ: &str = "eludite.terminal.read";
pub const WAIT: &str = "eludite.terminal.wait";
pub const RESIZE: &str = "eludite.terminal.resize";
pub const CLOSE: &str = "eludite.terminal.close";
pub const CLEAR: &str = "eludite.terminal.clear";

pub const ALL: [&str; 8] = [OPEN, LIST, SEND, READ, WAIT, RESIZE, CLOSE, CLEAR];

/// `wait`'s default and largest timeout.
pub const DEFAULT_WAIT_MS: u64 = 10_000;
pub const MAX_WAIT_MS: u64 = 60_000;
/// `read`'s default and largest `max_lines`.
pub const DEFAULT_LINES: usize = 200;
pub const MAX_LINES: usize = 10_000;
/// The most text `read` (`since`) answers, in bytes.
pub const READ_TEXT_MAX: usize = 256 * 1024;

/// (title, input schema, output schema, permission)
fn schemas(id: &str) -> (&'static str, &'static str, &'static str, PermissionClass) {
    use PermissionClass::*;
    macro_rules! s {
        ($name:literal) => {
            (
                include_str!(concat!(
                    "../../../protocol/schemas/terminal-",
                    $name,
                    ".input.json"
                )),
                include_str!(concat!(
                    "../../../protocol/schemas/terminal-",
                    $name,
                    ".output.json"
                )),
            )
        };
    }
    let (title, (input, output), class) = match id {
        OPEN => ("Terminal: New Terminal", s!("open"), Execute),
        LIST => ("Terminal: List", s!("list"), Read),
        SEND => ("Terminal: Send", s!("send"), Execute),
        READ => ("Terminal: Read", s!("read"), Read),
        WAIT => ("Terminal: Wait", s!("wait"), Read),
        RESIZE => ("Terminal: Resize", s!("resize"), Execute),
        CLOSE => ("Terminal: Kill Terminal", s!("close"), Execute),
        CLEAR => ("Terminal: Clear", s!("clear"), Execute),
        other => unreachable!("not a terminal command: {other}"),
    };
    (title, input, output, class)
}

/// `read`'s modes.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReadMode {
    #[default]
    Screen,
    Since,
    Scrollback,
}

/// A parsed, validated terminal command.
#[derive(Debug, Clone, PartialEq)]
pub enum TerminalRequest {
    Open {
        profile: Option<String>,
        cwd: Option<String>,
        env: BTreeMap<String, String>,
        name: Option<String>,
        split_with: Option<String>,
        replace: Option<String>,
    },
    List,
    Send {
        terminal: Option<String>,
        text: String,
        newline: bool,
    },
    Read {
        terminal: Option<String>,
        mode: ReadMode,
        mark: Option<u64>,
        max_lines: usize,
    },
    Wait {
        terminal: Option<String>,
        mark: Option<u64>,
        prompt: bool,
        /// Checked to compile.
        pattern: Option<String>,
        exit: bool,
        timeout_ms: u64,
    },
    Resize {
        terminal: Option<String>,
        cols: u16,
        rows: u16,
    },
    Close {
        terminal: Option<String>,
        kill: bool,
    },
    Clear {
        terminal: Option<String>,
    },
}

impl TerminalRequest {
    pub fn command(&self) -> &'static str {
        match self {
            TerminalRequest::Open { .. } => OPEN,
            TerminalRequest::List => LIST,
            TerminalRequest::Send { .. } => SEND,
            TerminalRequest::Read { .. } => READ,
            TerminalRequest::Wait { .. } => WAIT,
            TerminalRequest::Resize { .. } => RESIZE,
            TerminalRequest::Close { .. } => CLOSE,
            TerminalRequest::Clear { .. } => CLEAR,
        }
    }
}

// ----- Outputs -----

/// `terminal-open.output.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OpenOutput {
    pub terminal: String,
    pub pid: u32,
    pub name: String,
    pub profile: String,
    pub cwd: String,
    pub integration: bool,
    pub mark: u64,
}

/// A terminal of `terminal-list.output.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalRow {
    pub terminal: String,
    pub name: String,
    pub profile: String,
    pub pid: u32,
    pub cwd: String,
    pub running: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    pub busy: bool,
    pub integration: bool,
    pub cols: u16,
    pub rows: u16,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub split_with: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    pub mark: u64,
}

/// `terminal-list.output.json`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ListOutput {
    pub terminals: Vec<TerminalRow>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active: Option<String>,
    pub profiles: Vec<String>,
}

/// `terminal-send.output.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SendOutput {
    pub terminal: String,
    pub mark: u64,
    pub bytes: usize,
}

/// The cursor, 1-based.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Cursor {
    pub line: usize,
    pub column: usize,
}

/// `terminal-read.output.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReadOutput {
    pub terminal: String,
    pub text: String,
    pub cursor: Cursor,
    pub cols: u16,
    pub rows: u16,
    pub running: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    pub busy: bool,
    pub integration: bool,
    pub truncated: bool,
    pub mark: u64,
}

/// `terminal-wait.output.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WaitOutput {
    pub terminal: String,
    /// `prompt`, `pattern`, `exit`, `timeout` or `interrupted`.
    pub matched: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interrupted_by: Option<String>,
    pub text: String,
    #[serde(default, rename = "match", skip_serializing_if = "Option::is_none")]
    pub matched_text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    pub integration: bool,
    pub truncated: bool,
    pub elapsed_ms: u64,
    pub mark: u64,
}

/// `terminal-resize.output.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResizeOutput {
    pub terminal: String,
    pub cols: u16,
    pub rows: u16,
}

/// `terminal-close.output.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CloseOutput {
    pub terminal: String,
    pub killed: bool,
}

/// `terminal-clear.output.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClearOutput {
    pub terminal: String,
    pub mark: u64,
}

/// A terminal command's answer.
#[derive(Debug, Clone, PartialEq)]
pub enum TerminalOutput {
    Open(OpenOutput),
    List(ListOutput),
    Send(SendOutput),
    Read(ReadOutput),
    Wait(WaitOutput),
    Resize(ResizeOutput),
    Close(CloseOutput),
    Clear(ClearOutput),
}

impl TerminalOutput {
    pub fn to_json(&self) -> Value {
        match self {
            TerminalOutput::Open(o) => serde_json::to_value(o),
            TerminalOutput::List(o) => serde_json::to_value(o),
            TerminalOutput::Send(o) => serde_json::to_value(o),
            TerminalOutput::Read(o) => serde_json::to_value(o),
            TerminalOutput::Wait(o) => serde_json::to_value(o),
            TerminalOutput::Resize(o) => serde_json::to_value(o),
            TerminalOutput::Close(o) => serde_json::to_value(o),
            TerminalOutput::Clear(o) => serde_json::to_value(o),
        }
        .expect("terminal outputs serialize")
    }
}

/// Whatever owns the terminals (the shell). Called on the invoking thread, never the UI thread's for `wait`.
pub trait TerminalCommands: Send + Sync {
    fn apply(&self, request: TerminalRequest) -> Result<TerminalOutput, CommandError>;
}

// ----- Input -----

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct OpenIn {
    profile: Option<String>,
    cwd: Option<String>,
    env: Option<BTreeMap<String, String>>,
    name: Option<String>,
    split_with: Option<String>,
    replace: Option<String>,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct ListIn {}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct SendIn {
    terminal: Option<String>,
    text: Option<String>,
    newline: Option<bool>,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct ReadIn {
    terminal: Option<String>,
    mode: Option<ReadMode>,
    mark: Option<u64>,
    max_lines: Option<u64>,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct WaitIn {
    terminal: Option<String>,
    mark: Option<u64>,
    prompt: Option<bool>,
    pattern: Option<String>,
    exit: Option<bool>,
    timeout_ms: Option<u64>,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct ResizeIn {
    terminal: Option<String>,
    cols: Option<u64>,
    rows: Option<u64>,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct CloseIn {
    terminal: Option<String>,
    kill: Option<bool>,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct ClearIn {
    terminal: Option<String>,
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

/// A terminal id (`term1`).
fn terminal_id(name: &str, v: Option<String>) -> Result<Option<String>, CommandError> {
    match v {
        Some(t)
            if !t
                .strip_prefix("term")
                .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit())) =>
        {
            Err(invalid(format!(
                "`{name}` is a terminal id from eludite.terminal.list (`term1`), not `{t}`"
            )))
        }
        other => Ok(other),
    }
}

fn non_empty(name: &str, v: Option<String>) -> Result<Option<String>, CommandError> {
    match v {
        Some(s) if s.trim().is_empty() => Err(invalid(format!("`{name}` must not be empty"))),
        other => Ok(other),
    }
}

/// Parse and validate the input of terminal command `id`.
pub fn parse(id: &str, value: Value) -> Result<TerminalRequest, CommandError> {
    Ok(match id {
        OPEN => {
            let i: OpenIn = input(value)?;
            if i.split_with.is_some() && i.replace.is_some() {
                return Err(invalid("give `split_with` or `replace`, not both"));
            }
            let name = non_empty("name", i.name)?;
            if name.as_ref().is_some_and(|n| n.chars().count() > 100) {
                return Err(invalid("`name` is at most 100 characters"));
            }
            TerminalRequest::Open {
                profile: non_empty("profile", i.profile)?,
                cwd: non_empty("cwd", i.cwd)?,
                env: i.env.unwrap_or_default(),
                name,
                split_with: terminal_id("split_with", i.split_with)?,
                replace: terminal_id("replace", i.replace)?,
            }
        }
        LIST => {
            let _: ListIn = input(value)?;
            TerminalRequest::List
        }
        SEND => {
            let i: SendIn = input(value)?;
            let text = i.text.ok_or_else(|| invalid("`text` is required"))?;
            if text.len() > 65_536 {
                return Err(invalid("`text` is at most 65536 bytes"));
            }
            TerminalRequest::Send {
                terminal: terminal_id("terminal", i.terminal)?,
                text,
                newline: i.newline.unwrap_or(true),
            }
        }
        READ => {
            let i: ReadIn = input(value)?;
            let max_lines = match i.max_lines {
                None => DEFAULT_LINES,
                Some(n) if (1..=MAX_LINES as u64).contains(&n) => n as usize,
                Some(_) => return Err(invalid(format!("`max_lines` is 1 to {MAX_LINES}"))),
            };
            TerminalRequest::Read {
                terminal: terminal_id("terminal", i.terminal)?,
                mode: i.mode.unwrap_or_default(),
                mark: i.mark,
                max_lines,
            }
        }
        WAIT => {
            let i: WaitIn = input(value)?;
            let timeout_ms = match i.timeout_ms {
                None => DEFAULT_WAIT_MS,
                Some(n) if n <= MAX_WAIT_MS => n,
                Some(_) => return Err(invalid(format!("`timeout_ms` is 0 to {MAX_WAIT_MS}"))),
            };
            let pattern = non_empty("pattern", i.pattern)?;
            if let Some(p) = &pattern {
                if p.len() > 1000 {
                    return Err(invalid("`pattern` is at most 1000 bytes"));
                }
                regex::Regex::new(p)
                    .map_err(|e| invalid(format!("`pattern` is not a regular expression: {e}")))?;
            }
            let exit = i.exit.unwrap_or(false);
            // Waiting for the prompt is the default when nothing else is asked.
            let prompt = i.prompt.unwrap_or(pattern.is_none() && !exit);
            if !prompt && pattern.is_none() && !exit {
                return Err(invalid("wait for something: `prompt`, `pattern` or `exit`"));
            }
            TerminalRequest::Wait {
                terminal: terminal_id("terminal", i.terminal)?,
                mark: i.mark,
                prompt,
                pattern,
                exit,
                timeout_ms,
            }
        }
        RESIZE => {
            let i: ResizeIn = input(value)?;
            let cols = match i.cols {
                Some(n @ 2..=1000) => n as u16,
                Some(_) => return Err(invalid("`cols` is 2 to 1000")),
                None => return Err(invalid("`cols` is required")),
            };
            let rows = match i.rows {
                Some(n @ 1..=1000) => n as u16,
                Some(_) => return Err(invalid("`rows` is 1 to 1000")),
                None => return Err(invalid("`rows` is required")),
            };
            TerminalRequest::Resize {
                terminal: terminal_id("terminal", i.terminal)?,
                cols,
                rows,
            }
        }
        CLOSE => {
            let i: CloseIn = input(value)?;
            TerminalRequest::Close {
                terminal: terminal_id("terminal", i.terminal)?,
                kill: i.kill.unwrap_or(false),
            }
        }
        CLEAR => {
            let i: ClearIn = input(value)?;
            TerminalRequest::Clear {
                terminal: terminal_id("terminal", i.terminal)?,
            }
        }
        other => return Err(CommandError::UnknownCommand(other.to_owned())),
    })
}

/// The public description of command `id` (one of [`ALL`]).
pub fn spec(id: &str) -> CommandSpec {
    let (title, input, output, permission) = schemas(id);
    CommandSpec {
        id: CommandId::new(id).expect("valid id"),
        title: title.into(),
        input_schema: serde_json::from_str(input).expect("protocol schemas are valid JSON"),
        output_schema: serde_json::from_str(output).expect("protocol schemas are valid JSON"),
        permission,
        agent_visible: true,
    }
}

/// The escalation hook of terminal command `id`: the policy's `terminal.run` for the commands that run something
/// (with the running agent session's grant), `kill` dangerous. The reads have none.
pub fn escalation(id: &'static str) -> Option<EscalationHook> {
    if matches!(id, LIST | READ | WAIT) {
        return None;
    }
    Some(Arc::new(move |input: &Value, view: &PolicyView| {
        let kill = id == CLOSE && input.get("kill").and_then(Value::as_bool) == Some(true);
        let granted = view.session_granted(TERMINAL_RUN_GRANT);
        Some(view.terminal().decide(granted, kill))
    }))
}

/// Register every terminal command, applying them to `target`, with their escalation hooks.
pub fn register(registry: &CommandRegistry, target: Arc<dyn TerminalCommands>) {
    for id in ALL {
        let target = target.clone();
        registry.replace_with_escalation(spec(id), escalation(id), move |input| {
            let request = parse(id, input)?;
            target.apply(request).map(|out| out.to_json())
        });
    }
}

/// The escalation a hook gives, for tests of other crates.
pub fn decide(id: &'static str, input: &Value, view: &PolicyView) -> Option<Escalation> {
    escalation(id).and_then(|h| h(input, view))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::policy::{
        AgentPolicy, AlwaysAllow, PolicyRule, PolicySnapshot, RuleDecision, RunPolicy,
        TerminalPolicy, Verdict,
    };
    use crate::{CallClass, Caller, with_caller};
    use serde_json::json;

    /// `value` against `schema`: required members, no undeclared member, enums, patterns and types, through arrays.
    fn conforms(schema: &str, value: &Value) {
        let root: Value = serde_json::from_str(schema).unwrap();
        check(&root, value, "$");
    }

    fn check(schema: &Value, value: &Value, at: &str) {
        if let Some(e) = schema["enum"].as_array() {
            assert!(e.contains(value), "{at}: {value} not in {e:?}");
        }
        if let Some(p) = schema["pattern"].as_str() {
            let re = regex::Regex::new(p).unwrap();
            assert!(re.is_match(value.as_str().unwrap()), "{at}: {value} !~ {p}");
        }
        match schema["type"].as_str() {
            Some("object") => {
                let obj = value
                    .as_object()
                    .unwrap_or_else(|| panic!("{at}: not an object"));
                for r in schema["required"].as_array().into_iter().flatten() {
                    assert!(obj.contains_key(r.as_str().unwrap()), "{at}: missing {r}");
                }
                for (k, v) in obj {
                    let p = &schema["properties"][k];
                    assert!(!p.is_null(), "{at}: unexpected {k}");
                    check(p, v, &format!("{at}.{k}"));
                }
            }
            Some("array") => {
                for (i, v) in value.as_array().unwrap().iter().enumerate() {
                    check(&schema["items"], v, &format!("{at}[{i}]"));
                }
            }
            Some("integer") => {
                let n = value
                    .as_i64()
                    .unwrap_or_else(|| panic!("{at}: not an integer"));
                if let Some(min) = schema["minimum"].as_i64() {
                    assert!(n >= min, "{at}: {n} < {min}");
                }
            }
            Some("string") => assert!(value.is_string(), "{at}: not a string"),
            Some("boolean") => assert!(value.is_boolean(), "{at}: not a boolean"),
            _ => {}
        }
    }

    #[test]
    fn every_schema_parses_and_names_its_command() {
        for id in ALL {
            let s = spec(id);
            assert_eq!(s.input_schema["title"], format!("{id} input"));
            assert_eq!(s.output_schema["title"], format!("{id} output"));
            assert_eq!(s.input_schema["additionalProperties"], false, "{id}");
            assert!(s.agent_visible);
            let want = match id {
                LIST | READ | WAIT => PermissionClass::Read,
                _ => PermissionClass::Execute,
            };
            assert_eq!(s.permission, want, "{id}");
        }
    }

    #[test]
    fn parses_and_validates_input() {
        assert_eq!(
            parse(OPEN, json!({})).unwrap(),
            TerminalRequest::Open {
                profile: None,
                cwd: None,
                env: BTreeMap::new(),
                name: None,
                split_with: None,
                replace: None
            }
        );
        assert!(parse(OPEN, json!({"split_with": "term1", "replace": "term2"})).is_err());
        assert!(parse(OPEN, json!({"split_with": "t1"})).is_err());
        assert!(parse(OPEN, json!({"name": " "})).is_err());
        assert!(parse(OPEN, json!({"bogus": 1})).is_err());
        assert_eq!(
            parse(SEND, json!({"text": "ls"})).unwrap(),
            TerminalRequest::Send {
                terminal: None,
                text: "ls".into(),
                newline: true
            }
        );
        assert!(parse(SEND, json!({})).is_err(), "text is required");
        assert!(parse(SEND, json!({"text": "x".repeat(70_000)})).is_err());
        assert_eq!(
            parse(READ, Value::Null).unwrap(),
            TerminalRequest::Read {
                terminal: None,
                mode: ReadMode::Screen,
                mark: None,
                max_lines: DEFAULT_LINES
            }
        );
        assert!(parse(READ, json!({"max_lines": 0})).is_err());
        assert!(parse(READ, json!({"mode": "everything"})).is_err());
        // Waiting for the prompt is the default.
        assert_eq!(
            parse(WAIT, json!({"terminal": "term2"})).unwrap(),
            TerminalRequest::Wait {
                terminal: Some("term2".into()),
                mark: None,
                prompt: true,
                pattern: None,
                exit: false,
                timeout_ms: DEFAULT_WAIT_MS
            }
        );
        assert!(matches!(
            parse(WAIT, json!({"pattern": "done"})).unwrap(),
            TerminalRequest::Wait { prompt: false, .. }
        ));
        assert!(parse(WAIT, json!({"pattern": "("})).is_err());
        assert!(parse(WAIT, json!({"timeout_ms": 60_001})).is_err());
        assert!(parse(WAIT, json!({"prompt": false})).is_err());
        assert!(parse(RESIZE, json!({"cols": 1, "rows": 10})).is_err());
        assert!(parse(RESIZE, json!({"cols": 80})).is_err());
        assert_eq!(
            parse(CLOSE, json!({"kill": true})).unwrap(),
            TerminalRequest::Close {
                terminal: None,
                kill: true
            }
        );
        assert!(parse(CLEAR, json!({"terminal": "x"})).is_err());
        assert!(parse("eludite.terminal.nope", json!({})).is_err());
    }

    #[test]
    fn outputs_follow_their_schemas() {
        let row = TerminalRow {
            terminal: "term1".into(),
            name: "bash".into(),
            profile: "bash".into(),
            pid: 42,
            cwd: "/w".into(),
            running: false,
            exit_code: Some(0),
            busy: false,
            integration: true,
            cols: 80,
            rows: 24,
            split_with: Some("term2".into()),
            agent: Some("Claude".into()),
            mark: 10,
        };
        let cases: Vec<(&str, TerminalOutput)> = vec![
            (
                OPEN,
                TerminalOutput::Open(OpenOutput {
                    terminal: "term1".into(),
                    pid: 7,
                    name: "bash".into(),
                    profile: "bash".into(),
                    cwd: "/w".into(),
                    integration: true,
                    mark: 0,
                }),
            ),
            (
                LIST,
                TerminalOutput::List(ListOutput {
                    terminals: vec![row],
                    active: Some("term1".into()),
                    profiles: vec!["bash".into(), "sh".into()],
                }),
            ),
            (
                SEND,
                TerminalOutput::Send(SendOutput {
                    terminal: "term1".into(),
                    mark: 5,
                    bytes: 3,
                }),
            ),
            (
                READ,
                TerminalOutput::Read(ReadOutput {
                    terminal: "term1".into(),
                    text: "$ ".into(),
                    cursor: Cursor { line: 1, column: 3 },
                    cols: 80,
                    rows: 24,
                    running: true,
                    exit_code: None,
                    busy: false,
                    integration: false,
                    truncated: false,
                    mark: 2,
                }),
            ),
            (
                WAIT,
                TerminalOutput::Wait(WaitOutput {
                    terminal: "term1".into(),
                    matched: "interrupted".into(),
                    interrupted_by: Some("user".into()),
                    text: "x".into(),
                    matched_text: Some("x".into()),
                    exit_code: Some(1),
                    integration: true,
                    truncated: false,
                    elapsed_ms: 3,
                    mark: 9,
                }),
            ),
            (
                RESIZE,
                TerminalOutput::Resize(ResizeOutput {
                    terminal: "term1".into(),
                    cols: 100,
                    rows: 30,
                }),
            ),
            (
                CLOSE,
                TerminalOutput::Close(CloseOutput {
                    terminal: "term1".into(),
                    killed: true,
                }),
            ),
            (
                CLEAR,
                TerminalOutput::Clear(ClearOutput {
                    terminal: "term1".into(),
                    mark: 4,
                }),
            ),
        ];
        for (id, out) in cases {
            let (_, _, schema, _) = schemas(id);
            let v = out.to_json();
            conforms(schema, &v);
            if id == WAIT {
                assert_eq!(v["match"], "x", "`match` in the JSON");
            }
        }
    }

    fn view(policy: Option<TerminalPolicy>, grants: &[&str], rules: Vec<PolicyRule>) -> PolicyView {
        PolicyView::of(PolicySnapshot {
            policy: AgentPolicy {
                terminal: policy,
                rules,
                ..Default::default()
            },
            session_grants: grants.iter().map(|g| (*g).to_owned()).collect(),
            ..Default::default()
        })
    }

    #[test]
    fn the_terminal_policy_object_applies() {
        let send = json!({"text": "ls"});
        // Reads never escalate.
        for id in [LIST, READ, WAIT] {
            assert!(escalation(id).is_none(), "{id}");
        }
        // prompt (default): dangerous, Always Allow is \"for this session\".
        match decide(SEND, &send, &view(None, &[], vec![])) {
            Some(Escalation::Raise {
                class,
                reason,
                always_allow,
            }) => {
                assert_eq!(class, PermissionClass::Dangerous);
                assert!(reason.contains("terminal.run: prompt"));
                assert_eq!(
                    always_allow,
                    AlwaysAllow::Session(TERMINAL_RUN_GRANT.into())
                );
            }
            other => panic!("{other:?}"),
        }
        // Once granted: the call runs (Granted) at its class.
        let granted = decide(OPEN, &json!({}), &view(None, &[TERMINAL_RUN_GRANT], vec![]));
        assert!(matches!(
            granted,
            Some(Escalation::Raise {
                class: PermissionClass::Execute,
                always_allow: AlwaysAllow::Granted,
                ..
            })
        ));
        // allow: granted without asking; deny: refused, naming the policy.
        let allow = Some(TerminalPolicy {
            run: Some(RunPolicy::Allow),
        });
        assert!(matches!(
            decide(SEND, &send, &view(allow.clone(), &[], vec![])),
            Some(Escalation::Raise {
                always_allow: AlwaysAllow::Granted,
                ..
            })
        ));
        let deny = Some(TerminalPolicy {
            run: Some(RunPolicy::Deny),
        });
        assert_eq!(
            decide(SEND, &send, &view(deny, &[TERMINAL_RUN_GRANT], vec![])),
            Some(Escalation::Refuse(
                "the solution's policy sets terminal.run to deny".into()
            ))
        );
        // A kill is dangerous even when allowed.
        assert!(matches!(
            decide(CLOSE, &json!({"kill": true}), &view(allow, &[], vec![])),
            Some(Escalation::Raise {
                class: PermissionClass::Dangerous,
                always_allow: AlwaysAllow::Never,
                ..
            })
        ));
    }

    struct Fake;
    impl TerminalCommands for Fake {
        fn apply(&self, request: TerminalRequest) -> Result<TerminalOutput, CommandError> {
            Ok(TerminalOutput::Send(SendOutput {
                terminal: "term1".into(),
                mark: 0,
                bytes: match request {
                    TerminalRequest::Send { text, .. } => text.len(),
                    _ => 0,
                },
            }))
        }
    }

    #[test]
    fn registered_calls_classify_and_the_gate_decides_granted_ones_without_asking() {
        let registry = CommandRegistry::new();
        register(&registry, Arc::new(Fake));
        let send = json!({"text": "ls"});
        // The default policy: the first send asks (dangerous).
        let first = registry.classify(SEND, &send).unwrap();
        assert_eq!(first.class, PermissionClass::Dangerous);
        let p = AgentPolicy::default();
        assert_eq!(
            p.decide_call(&first, "eludite-terminal-send", &send),
            Verdict::Ask
        );
        // Granted for the session: execute, allowed by the policy without asking; a deny rule still refuses.
        registry.set_policy_source(Arc::new(|| PolicySnapshot {
            session_grants: vec![TERMINAL_RUN_GRANT.into()],
            ..Default::default()
        }));
        let later = registry.classify(SEND, &send).unwrap();
        assert_eq!(later.class, PermissionClass::Execute);
        assert_eq!(later.always_allow, AlwaysAllow::Granted);
        assert!(matches!(
            p.decide_call(&later, "eludite-terminal-send", &send),
            Verdict::Allow(r) if r.contains("this agent session")
        ));
        let denying = AgentPolicy {
            rules: vec![PolicyRule {
                tool: "eludite-terminal-send".into(),
                command_prefix: None,
                decision: RuleDecision::Deny,
            }],
            ..Default::default()
        };
        assert!(matches!(
            denying.decide_call(&later, "mcp__eludite__eludite-terminal-send", &send),
            Verdict::Deny(_)
        ));
        // Reads are read; the call reaches the target.
        assert_eq!(
            registry.classify(READ, &json!({})).unwrap(),
            CallClass::declared(PermissionClass::Read)
        );
        let out = with_caller(Caller::User, || registry.invoke(SEND, send.clone())).unwrap();
        assert_eq!(out["bytes"], 2);
    }

    #[test]
    fn the_policy_file_round_trips_the_terminal_object() {
        let p: AgentPolicy =
            serde_json::from_str(r#"{"version": 1, "terminal": {"run": "allow"}}"#).unwrap();
        assert_eq!(
            p.terminal,
            Some(TerminalPolicy {
                run: Some(RunPolicy::Allow)
            })
        );
        let schema: Value =
            serde_json::from_str(include_str!("../../../protocol/schemas/agents-policy.json"))
                .unwrap();
        let props = &schema["properties"]["terminal"]["properties"];
        assert_eq!(props["run"]["enum"], json!(["prompt", "allow", "deny"]));
        assert!(
            serde_json::from_str::<AgentPolicy>(r#"{"version": 1, "terminal": {"x": 1}}"#).is_err()
        );
        let mut remembered = p.clone();
        let session = CallClass {
            class: PermissionClass::Dangerous,
            reason: None,
            always_allow: AlwaysAllow::Session(TERMINAL_RUN_GRANT.into()),
            refused: None,
        };
        assert_eq!(
            remembered.remember(&session, "t", &json!({})),
            None,
            "nothing written"
        );
        assert_eq!(remembered, p);
    }
}
