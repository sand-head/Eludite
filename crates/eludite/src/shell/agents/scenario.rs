//! Brief 0030: the scripted fake agent's debugging scenario, proposal 0001's proving scenario in its scripted form.
//!
//! [`DebugAgent`] is a [`Planner`] for the fake agent's `planned` scenario: given the prompt "the program `<path>`
//! fails its self-check" ([`prompt`]), the tool list, the debugging guide and the answers so far, it picks the next
//! Eludite tool call the way the guide tells a model to, using only what a model would see:
//!
//! 1. `eludite.debug.start` on the project with `wait_ms`. A start answers once the program runs (`mode: running`),
//!    so `eludite.debug.wait` (`until: stopped`, `depth: 2`; its description calls it the way to wait after starting)
//!    follows it. Either the program ran to its end (`mode: design`, an `exit_code`, and the check's
//!    `FAIL Type.Method: expected …, actual …` line in `output`), or it stopped on an unhandled exception
//!    (`stopped.reason: exception`, its location and the locals two levels deep in the summary).
//! 2. Where to break, without running anything: after an exit, the function the check names, found in the project's
//!    `Program.cs` with `eludite.file.open` and `eludite.editor.find` (` Method(`, its declaration); after an exception,
//!    the locals show which member of a local is null, `eludite.editor.find` checks which `local.Member.` the stopped
//!    line dereferences, and finds that type's constructor (`public Type(`).
//! 3. `eludite.debug.toggle_breakpoint` (`set`, `remove_after: true`, as the guide's cleanup rule says) on the
//!    function's first statement: the line after the declaration's opening brace (the corpus is written in the C#
//!    convention, the brace on its own line). Its compact answer (brief 0034) must name that line.
//! 4. `eludite.debug.start` again (after an exit) or `eludite.debug.restart` (from the exception's break), then `wait`
//!    for the breakpoint with `depth: 2` (its summary has the locals: brief 0034 drops brief 0030's `snapshot` after
//!    it), then [`DebugAgent::steps`] `eludite.debug.step_over`s quoting the stop they act on (`depth: 2`): each
//!    scenario's one number, how far the faulting statement is from the function's first statement.
//! 5. The answer: the statement's location and the locals of the last summary. The session is left at that stop.
//!
//! The debug calls this makes with one step: six (start, wait, toggle, start or restart, wait, step); the proving
//! threshold is eight. A start or restart that answers settled (a break or the end) is not followed by a `wait`.
//!
//! Brief 0036: [`DebugAgent::with_condition`] sets the breakpoint with a `condition`, and a replacement for it. When the
//! run ends without stopping and the answer's `breakpoints_failed` names the breakpoint (the adapter rejected the
//! condition), the planner reads the reason ([`DebugAgent::failures_read`]), sets the breakpoint again with the
//! replacement and runs again: three more debug calls, as a model that reads the answer would make.

use std::path::Path;

use eludite_acp::fake_agent::{Next, Planner, Seen, Step};
use serde_json::{Value, json};

/// The tools the scenario needs, as MCP names them; it answers without calling anything when one is missing.
pub const TOOLS: [&str; 7] = [
    "eludite-debug-start",
    "eludite-debug-wait",
    "eludite-debug-toggle_breakpoint",
    "eludite-debug-step_over",
    "eludite-debug-restart",
    "eludite-file-open",
    "eludite-editor-find",
];

/// How long each resuming call waits for the program to settle (a start includes the adapter's launch).
const WAIT_MS: u64 = 20_000;

/// The proving scenario's prompt for the program at `project` (proposal 0001 section 1, brief 0030's wording).
pub fn prompt(project: &Path) -> String {
    format!(
        "The program `{}` fails its self-check when run. Debug it with Eludite's tools, find the statement that \
         produces the wrong value, and tell me the statement, the line and the local variable values that show it. \
         Do not edit files.",
        project.display()
    )
}

/// What the program did when first started.
#[derive(Debug, Clone, PartialEq)]
enum Failure {
    /// It exited; the check named this function (`Basket.Total`).
    Check { function: String },
    /// It stopped on an exception at this line; the candidates are (local, member, the local's type) whose member
    /// was null in the snapshot.
    Exception {
        line: u64,
        candidates: Vec<(String, String, String)>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    Start,
    Started,
    Waited,
    Opened,
    FoundDeref,
    FoundDeclaration,
    Toggled,
    Rerun,
    RerunWaited,
    Stepped,
}

/// The planner (see the module docs). After the run, [`DebugAgent::last`] is the last stop summary it received.
#[derive(Debug)]
pub struct DebugAgent {
    /// How many `step_over`s after the breakpoint's stop.
    pub steps: usize,
    phase: Phase,
    project: String,
    source: String,
    failure: Option<Failure>,
    /// The candidate being checked with a find (exception path).
    candidate: Option<(String, String, String)>,
    breakpoint: u64,
    stepped: usize,
    /// The last stop summary received.
    pub last: Option<Value>,
    /// Every call made, with its answer's size and time, as of the last decision.
    pub calls: Vec<Step>,
    /// Why the planner gave up, when it did.
    pub gave_up: Option<String>,
    /// The breakpoint's condition, and the one to set instead when the adapter rejects it (brief 0036).
    condition: Option<String>,
    replacement: Option<String>,
    /// The reasons read in `breakpoints_failed` (brief 0036).
    pub failures_read: Vec<String>,
}

impl DebugAgent {
    pub fn new(steps: usize) -> Self {
        Self {
            steps,
            phase: Phase::Start,
            project: String::new(),
            source: String::new(),
            failure: None,
            candidate: None,
            breakpoint: 0,
            stepped: 0,
            last: None,
            calls: Vec::new(),
            gave_up: None,
            condition: None,
            replacement: None,
            failures_read: Vec::new(),
        }
    }

    /// Set the breakpoint with `condition`, and with `replacement` (empty: none) if the adapter rejects it.
    pub fn with_condition(mut self, condition: &str, replacement: &str) -> Self {
        self.condition = Some(condition.to_owned());
        self.replacement = Some(replacement.to_owned());
        self
    }

    fn toggle(&mut self) -> Next {
        let mut args = json!({"path": self.source, "line": self.breakpoint, "action": "set", "remove_after": true});
        if let Some(c) = &self.condition {
            args["condition"] = json!(c);
        }
        self.call(Phase::Toggled, "eludite-debug-toggle_breakpoint", args)
    }

    fn call(&mut self, phase: Phase, tool: &str, arguments: Value) -> Next {
        self.phase = phase;
        Next::Call {
            tool: tool.to_owned(),
            arguments,
        }
    }

    fn give_up(&mut self, why: String) -> Next {
        self.gave_up = Some(why.clone());
        Next::Answer(format!("I could not finish: {why}"))
    }

    /// The first call: check the tools and the guide, read the project from the prompt, start it.
    fn begin(&mut self, seen: &Seen) -> Next {
        let names: Vec<&str> = seen
            .tools
            .iter()
            .filter_map(|t| t["name"].as_str())
            .collect();
        if let Some(missing) = TOOLS.iter().find(|t| !names.contains(t)) {
            return self.give_up(format!("Eludite has no `{missing}` tool"));
        }
        // The guide's first rule (read `snapshot` before acting) and its third (quote `stop`) are what this follows.
        if !seen.guide.contains("snapshot") || !seen.guide.contains("`stop`") {
            return self.give_up("the debugging guide was not readable".into());
        }
        let Some(project) = seen.prompt.split('`').nth(1).filter(|p| !p.is_empty()) else {
            return self.give_up("the prompt names no program".into());
        };
        self.project = project.to_owned();
        self.call(
            Phase::Started,
            "eludite-debug-start",
            json!({"project": self.project, "wait_ms": WAIT_MS}),
        )
    }

    /// A start's answer: settled (a break, the end), or running, when `wait` follows.
    fn settled(r: &Value) -> bool {
        matches!(r["mode"].as_str(), Some("break" | "design"))
    }

    fn wait(&mut self, phase: Phase) -> Next {
        self.call(
            phase,
            "eludite-debug-wait",
            json!({"until": "stopped", "wait_ms": WAIT_MS, "depth": 2}),
        )
    }

    fn started(&mut self, r: &Value) -> Next {
        self.last = Some(r.clone());
        if Self::settled(r) {
            self.waited(r)
        } else {
            self.wait(Phase::Waited)
        }
    }

    /// The first run's end: an exit with the check's line, or an exception stop.
    fn waited(&mut self, r: &Value) -> Next {
        self.last = Some(r.clone());
        if r["mode"] == "break" && r["stopped"]["reason"] == "exception" {
            let location = &r["stopped"]["location"];
            let (Some(path), Some(line)) = (location["path"].as_str(), location["line"].as_u64())
            else {
                return self.give_up(format!("the exception stop has no source: {r}"));
            };
            self.source = path.to_owned();
            self.failure = Some(Failure::Exception {
                line,
                candidates: Vec::new(),
            });
            return self.exception_locals(r);
        }
        if r["mode"] != "design" {
            return self.give_up(format!(
                "the program neither exited nor threw: {}",
                r["mode"]
            ));
        }
        let lines: Vec<&str> = r["output"]["lines"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|l| l["text"].as_str())
            .collect();
        let Some(fail) = lines.iter().find_map(|l| l.trim().strip_prefix("FAIL ")) else {
            return self.give_up(format!(
                "the program exited with {} and printed no check failure: {lines:?}",
                r["exit_code"]
            ));
        };
        // `Basket.Total: expected …` or `Coins.Cents(Quarter): expected …`.
        let function = fail
            .split(':')
            .next()
            .unwrap_or_default()
            .split('(')
            .next()
            .unwrap_or_default()
            .trim()
            .to_owned();
        if function.is_empty() {
            return self.give_up(format!("no function in `{fail}`"));
        }
        self.failure = Some(Failure::Check { function });
        // A console program's code is in its project folder's Program.cs.
        let dir = Path::new(&self.project)
            .parent()
            .unwrap_or(Path::new("."))
            .to_path_buf();
        self.source = dir.join("Program.cs").to_string_lossy().into_owned();
        self.call(
            Phase::Opened,
            "eludite-file-open",
            json!({"path": self.source}),
        )
    }

    /// At the exception's stop: the locals whose members are null.
    fn exception_locals(&mut self, r: &Value) -> Next {
        let mut found = Vec::new();
        for row in r["locals"]["rows"].as_array().into_iter().flatten() {
            for child in row["children"].as_array().into_iter().flatten() {
                // One spelling on every adapter (brief 0034).
                if child["value"].as_str() == Some("null") {
                    found.push((
                        row["name"].as_str().unwrap_or_default().to_owned(),
                        child["name"].as_str().unwrap_or_default().to_owned(),
                        row["type"].as_str().unwrap_or_default().to_owned(),
                    ));
                }
            }
        }
        if found.is_empty() {
            return self.give_up(format!("no local has a null member: {}", r["locals"]));
        }
        if let Some(Failure::Exception { candidates, .. }) = &mut self.failure {
            *candidates = found;
        }
        self.call(
            Phase::Opened,
            "eludite-file-open",
            json!({"path": self.source}),
        )
    }

    /// Find the next null member's dereference (`local.Member.`), or give up when none is on the stopped line.
    fn next_deref(&mut self) -> Next {
        let candidate = match &mut self.failure {
            Some(Failure::Exception { candidates, .. }) if !candidates.is_empty() => {
                candidates.remove(0)
            }
            _ => return self.give_up("no null member is dereferenced on the stopped line".into()),
        };
        let query = format!("{}.{}.", candidate.0, candidate.1);
        self.candidate = Some(candidate);
        self.call(
            Phase::FoundDeref,
            "eludite-editor-find",
            json!({"path": self.source, "query": query, "case_sensitive": true}),
        )
    }

    fn opened(&mut self) -> Next {
        match self.failure.clone() {
            Some(Failure::Check { function }) => {
                let method = function.rsplit('.').next().unwrap_or_default().to_owned();
                self.call(
                    Phase::FoundDeclaration,
                    "eludite-editor-find",
                    json!({"path": self.source, "query": format!(" {method}("), "case_sensitive": true}),
                )
            }
            Some(Failure::Exception { .. }) => self.next_deref(),
            None => self.give_up("no failure to look at".into()),
        }
    }

    /// The find of `local.Member.`: on the stopped line, that member is the null one; find its type's constructor.
    fn found_deref(&mut self, r: &Value) -> Next {
        let stopped_line = match &self.failure {
            Some(Failure::Exception { line, .. }) => *line,
            _ => 0,
        };
        if r["found"] != true || r["line"].as_u64() != Some(stopped_line) {
            return self.next_deref();
        }
        let ty = self
            .candidate
            .as_ref()
            .map(|c| c.2.rsplit(['.', '+']).next().unwrap_or_default().to_owned())
            .unwrap_or_default();
        if ty.is_empty() {
            return self.give_up("the null member's owner has no type".into());
        }
        self.call(
            Phase::FoundDeclaration,
            "eludite-editor-find",
            json!({"path": self.source, "query": format!("public {ty}("), "case_sensitive": true}),
        )
    }

    /// The declaration's line: break on its first statement, two lines down (after the opening brace).
    fn found_declaration(&mut self, r: &Value) -> Next {
        let Some(line) = r["line"].as_u64().filter(|_| r["found"] == true) else {
            return self.give_up(format!("the declaration was not found: {r}"));
        };
        self.breakpoint = line + 2;
        self.toggle()
    }

    fn toggled(&mut self, r: &Value) -> Next {
        // The compact answer: the breakpoint's row (brief 0034).
        if !matches!(r["action"].as_str(), Some("added" | "changed"))
            || r["breakpoint"]["line"].as_u64() != Some(self.breakpoint)
        {
            return self.give_up(format!(
                "the breakpoint on line {} was not set: {r}",
                self.breakpoint
            ));
        }
        match self.failure {
            // The exception's session is still at its break: start it again from there.
            Some(Failure::Exception { .. }) => self.call(
                Phase::Rerun,
                "eludite-debug-restart",
                json!({ "wait_ms": WAIT_MS }),
            ),
            _ => self.call(
                Phase::Rerun,
                "eludite-debug-start",
                json!({"project": self.project, "wait_ms": WAIT_MS}),
            ),
        }
    }

    fn rerun(&mut self, r: &Value) -> Next {
        self.last = Some(r.clone());
        if Self::settled(r) {
            self.rerun_waited(r)
        } else {
            self.wait(Phase::RerunWaited)
        }
    }

    fn rerun_waited(&mut self, r: &Value) -> Next {
        self.last = Some(r.clone());
        // The breakpoint never stopped: the answer says why when the adapter rejected it (brief 0036).
        let failed = r["breakpoints_failed"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|f| f["line"].as_u64() == Some(self.breakpoint))
            .and_then(|f| f["message"].as_str())
            .map(str::to_owned);
        if r["mode"] != "break"
            && let Some(message) = failed
        {
            self.failures_read.push(message.clone());
            return match self.replacement.take() {
                Some(c) => {
                    self.condition = Some(c);
                    self.toggle()
                }
                None => self.give_up(format!(
                    "the breakpoint on line {} never stopped: {message}",
                    self.breakpoint
                )),
            };
        }
        if r["mode"] != "break" {
            return self.give_up(format!(
                "the breakpoint on line {} was not reached: {}",
                self.breakpoint, r["mode"]
            ));
        }
        // The wait's summary has the locals two deep: no snapshot after it.
        self.step(r)
    }

    fn step(&mut self, r: &Value) -> Next {
        self.last = Some(r.clone());
        if r["mode"] != "break" {
            return self.give_up(format!("the program left break mode: {}", r["mode"]));
        }
        if self.stepped >= self.steps {
            return Next::Answer(answer(r));
        }
        self.stepped += 1;
        self.call(
            Phase::Stepped,
            "eludite-debug-step_over",
            json!({"stop": r["stop"], "wait_ms": 5000, "depth": 2}),
        )
    }
}

impl Planner for DebugAgent {
    fn next(&mut self, seen: &Seen) -> Next {
        self.calls = seen.steps.clone();
        let Some(last) = seen.steps.last() else {
            return self.begin(seen);
        };
        let r = match &last.result {
            Ok(r) => r.clone(),
            Err(e) => return self.give_up(format!("{} failed: {e}", last.tool)),
        };
        match self.phase {
            Phase::Start => self.begin(seen),
            Phase::Started => self.started(&r),
            Phase::Waited => self.waited(&r),
            Phase::Opened => self.opened(),
            Phase::FoundDeref => self.found_deref(&r),
            Phase::FoundDeclaration => self.found_declaration(&r),
            Phase::Toggled => self.toggled(&r),
            Phase::Rerun => self.rerun(&r),
            Phase::RerunWaited => self.rerun_waited(&r),
            Phase::Stepped => self.step(&r),
        }
    }
}

/// The answer from the last stop summary: the statement's location and the locals, two levels deep.
fn answer(r: &Value) -> String {
    let location = &r["stopped"]["location"];
    let mut s = format!(
        "The wrong value comes from the statement at {}:{} in {}. Its locals there:",
        Path::new(location["path"].as_str().unwrap_or_default())
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default(),
        location["line"],
        location["function"].as_str().unwrap_or_default()
    );
    fn rows(out: &mut String, list: &Value, prefix: &str) {
        for row in list.as_array().into_iter().flatten() {
            let name = format!("{prefix}{}", row["name"].as_str().unwrap_or_default());
            out.push_str(&format!(
                "\n- {name} = {}",
                row["value"].as_str().unwrap_or_default()
            ));
            rows(out, &row["children"], &format!("{name}."));
        }
    }
    rows(&mut s, &r["locals"]["rows"], "");
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn step(tool: &str, result: Value) -> Step {
        Step {
            tool: tool.into(),
            arguments: json!({}),
            result: Ok(result),
            bytes: 0,
            ms: 0.,
        }
    }

    fn seen() -> Seen {
        Seen {
            prompt: prompt(Path::new("/c/OffByOne/OffByOne.csproj")),
            tools: TOOLS.iter().map(|t| json!({ "name": t })).collect(),
            guide: "Call `eludite.debug.snapshot` first. Pass `stop` on every resuming call."
                .into(),
            steps: Vec::new(),
        }
    }

    fn call(next: Next) -> (String, Value) {
        match next {
            Next::Call { tool, arguments } => (tool, arguments),
            Next::Answer(a) => panic!("answered: {a}"),
        }
    }

    #[test]
    fn a_check_failure_breaks_in_the_function_it_names() {
        // The project's folder joined with Program.cs, in this platform's separators.
        let program = Path::new("/c/OffByOne")
            .join("Program.cs")
            .to_string_lossy()
            .into_owned();
        let mut a = DebugAgent::new(1);
        let mut s = seen();
        let (tool, args) = call(a.next(&s));
        assert_eq!(tool, "eludite-debug-start");
        assert_eq!(args["project"], "/c/OffByOne/OffByOne.csproj");
        s.steps
            .push(step(&tool, json!({"mode": "running", "stop": 0})));
        let (tool, args) = call(a.next(&s));
        assert_eq!(
            (tool.as_str(), &args["until"]),
            ("eludite-debug-wait", &json!("stopped"))
        );
        s.steps.push(step(
            &tool,
            json!({"mode": "design", "exit_code": 1, "output": {"lines": [{"seq": 0, "text": "FAIL Basket.Total: expected 75, actual 54"}]}}),
        ));
        let (tool, args) = call(a.next(&s));
        assert_eq!(
            (tool.as_str(), &args["path"]),
            ("eludite-file-open", &json!(program))
        );
        s.steps.push(step(&tool, json!({})));
        let (tool, args) = call(a.next(&s));
        assert_eq!(
            (tool.as_str(), &args["query"]),
            ("eludite-editor-find", &json!(" Total("))
        );
        s.steps.push(step(
            &tool,
            json!({"path": program, "found": true, "line": 10}),
        ));
        let (tool, args) = call(a.next(&s));
        assert_eq!(tool, "eludite-debug-toggle_breakpoint");
        assert_eq!(
            (&args["line"], &args["remove_after"]),
            (&json!(12), &json!(true))
        );
        s.steps.push(step(
            &tool,
            json!({"action": "added", "verified": false, "breakpoints_total": 1,
                   "breakpoint": {"kind": "line", "path": program, "line": 12, "enabled": true, "verified": false, "hits": 0}}),
        ));
        let (tool, _) = call(a.next(&s));
        assert_eq!(tool, "eludite-debug-start");
        s.steps
            .push(step(&tool, json!({"mode": "running", "stop": 0})));
        let (tool, args) = call(a.next(&s));
        assert_eq!(
            (tool.as_str(), &args["depth"]),
            ("eludite-debug-wait", &json!(2))
        );
        // The wait's summary has the locals: the step follows it at once.
        s.steps
            .push(step(&tool, json!({"mode": "break", "stop": 1})));
        let (tool, args) = call(a.next(&s));
        assert_eq!(
            (tool.as_str(), &args["stop"]),
            ("eludite-debug-step_over", &json!(1))
        );
        s.steps.push(step(
            &tool,
            json!({"mode": "break", "stop": 2, "stopped": {"location": {"path": program, "line": 13, "function": "Total"}},
                   "locals": {"rows": [{"name": "prices", "value": "Count = 5", "children": [{"name": "Count", "value": "5"}]}]}}),
        ));
        let Next::Answer(text) = a.next(&s) else {
            panic!("no answer")
        };
        assert!(
            text.starts_with("The wrong value comes from the statement at Program.cs:13 in Total."),
            "{text}"
        );
        assert!(
            text.contains("\n- prices = Count = 5\n- prices.Count = 5"),
            "{text}"
        );
    }

    #[test]
    fn an_exception_breaks_in_the_constructor_of_the_null_member() {
        let mut a = DebugAgent::new(1);
        let mut s = seen();
        // A start that answers settled (here at the exception) is not followed by a wait.
        let (tool, _) = call(a.next(&s));
        s.steps.push(step(
            &tool,
            json!({"mode": "break", "stop": 1, "stopped": {"reason": "exception", "location": {"path": "/c/P.cs", "line": 33, "function": "Describe"}},
                   "locals": {"rows": [
                {"name": "f", "type": "N.Folder", "value": "{N.Folder}", "children": [
                    {"name": "Parent", "value": "null"}, {"name": "Path", "value": "null"}]}]}}),
        ));
        let (tool, _) = call(a.next(&s));
        assert_eq!(tool, "eludite-file-open");
        s.steps.push(step(&tool, json!({})));
        let (tool, args) = call(a.next(&s));
        assert_eq!(args["query"], "f.Parent.");
        s.steps.push(step(&tool, json!({"found": false})));
        let (tool, args) = call(a.next(&s));
        assert_eq!(args["query"], "f.Path.");
        s.steps
            .push(step(&tool, json!({"found": true, "line": 33})));
        let (tool, args) = call(a.next(&s));
        assert_eq!(args["query"], "public Folder(");
        s.steps
            .push(step(&tool, json!({"found": true, "line": 13})));
        let (tool, args) = call(a.next(&s));
        assert_eq!(
            (tool.as_str(), &args["line"]),
            ("eludite-debug-toggle_breakpoint", &json!(15))
        );
        s.steps.push(step(
            &tool,
            json!({"action": "added", "verified": false, "pending": true, "session": 1, "breakpoints_total": 1,
                   "breakpoint": {"kind": "line", "path": "/c/P.cs", "line": 15, "enabled": true, "verified": false, "hits": 0}}),
        ));
        let (tool, _) = call(a.next(&s));
        assert_eq!(tool, "eludite-debug-restart");
        // A failed call ends the turn with what failed.
        s.steps.push(Step {
            result: Err("refused".into()),
            ..step(&tool, json!({}))
        });
        let Next::Answer(text) = a.next(&s) else {
            panic!("no answer")
        };
        assert_eq!(
            text,
            "I could not finish: eludite-debug-restart failed: refused"
        );
    }

    #[test]
    fn a_breakpoint_on_another_line_ends_the_turn() {
        let mut a = DebugAgent::new(1);
        a.phase = Phase::Toggled;
        a.breakpoint = 12;
        let mut s = seen();
        s.steps.push(step(
            "eludite-debug-toggle_breakpoint",
            json!({"action": "deleted", "verified": false, "breakpoints_total": 0}),
        ));
        let Next::Answer(text) = a.next(&s) else {
            panic!("no answer")
        };
        assert!(
            text.starts_with("I could not finish: the breakpoint on line 12 was not set"),
            "{text}"
        );
    }

    #[test]
    fn a_rejected_condition_is_read_and_replaced() {
        let mut a =
            DebugAgent::new(1).with_condition("coin == Money.Quarter", "coin == Coin.Quarter");
        a.phase = Phase::FoundDeclaration;
        a.source = "/c/P.cs".into();
        a.project = "/c/P.csproj".into();
        a.failure = Some(Failure::Check {
            function: "Coins.Cents".into(),
        });
        let mut s = seen();
        s.steps.push(step(
            "eludite-editor-find",
            json!({"found": true, "line": 17}),
        ));
        let (tool, args) = call(a.next(&s));
        assert_eq!(
            (tool.as_str(), &args["condition"]),
            (
                "eludite-debug-toggle_breakpoint",
                &json!("coin == Money.Quarter")
            )
        );
        s.steps.push(step(
            &tool,
            json!({"action": "added", "verified": false, "breakpoints_total": 1,
                   "breakpoint": {"kind": "line", "path": "/c/P.cs", "line": 19, "enabled": true, "verified": false, "hits": 0}}),
        ));
        let (tool, _) = call(a.next(&s));
        s.steps
            .push(step(&tool, json!({"mode": "running", "stop": 0})));
        let (tool, _) = call(a.next(&s));
        assert_eq!(tool, "eludite-debug-wait");
        s.steps.push(step(
            &tool,
            json!({"mode": "design", "exit_code": 1,
                   "breakpoints_failed": [{"path": "/c/P.cs", "line": 19, "session": 2, "message": "Unknown identifier: Money"}]}),
        ));
        let (tool, args) = call(a.next(&s));
        assert_eq!(
            (tool.as_str(), &args["condition"]),
            (
                "eludite-debug-toggle_breakpoint",
                &json!("coin == Coin.Quarter")
            )
        );
        assert_eq!(a.failures_read, ["Unknown identifier: Money"]);
        // Rejected again, it gives up with the reason.
        a.phase = Phase::RerunWaited;
        s.steps.push(step(
            "eludite-debug-wait",
            json!({"mode": "design", "breakpoints_failed": [{"line": 19, "session": 3, "message": "Unknown identifier: Coin"}]}),
        ));
        let Next::Answer(text) = a.next(&s) else {
            panic!("no answer")
        };
        assert_eq!(
            text,
            "I could not finish: the breakpoint on line 19 never stopped: Unknown identifier: Coin"
        );
    }

    #[test]
    fn it_needs_the_tools_and_the_guide() {
        let mut s = seen();
        s.tools.pop();
        assert_eq!(
            DebugAgent::new(1).next(&s),
            Next::Answer("I could not finish: Eludite has no `eludite-editor-find` tool".into())
        );
        let mut s = seen();
        s.guide.clear();
        assert!(matches!(DebugAgent::new(1).next(&s), Next::Answer(_)));
    }
}
