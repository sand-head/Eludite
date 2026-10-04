//! Find in Files and Replace in Files (brief 0042): `eludite.search.*`, what the Find in Files and Replace in Files
//! dialogs and the Find Results 1 and 2 windows do, for the person and agents alike: `find` (Find All), `replace`
//! (Replace All), `cancel` (the results window's Stop) and `results` (the window's results, paged; F8 and Shift+F8
//! with `navigate`).
//!
//! The schemas are `protocol/schemas/search-*.json` (checked in first, CLAUDE.md invariant 4). This module parses
//! input into a typed [`SearchRequest`], caps it (`max_results` at most [`MAX_RESULTS`]), serializes the typed
//! outputs and declares each command's class. The shell implements [`SearchCommands`] over `eludite-search`, on the
//! invoking thread: a search blocks its caller (an agent's thread, or a background task for the person), never the UI
//! thread.
//!
//! Classes: `find`, `cancel` and `results` are read (always allowed for agents). `replace` is edit (with `preview`,
//! the default, its replacements are held as pending changes for review and nothing is written until they are
//! accepted); `preview: false` writes at once and is raised to execute, so the policy's `execute` decides for agents.

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::policy::PolicyView;
use crate::{
    CommandError, CommandId, CommandRegistry, CommandSpec, Escalation, EscalationHook,
    PermissionClass,
};

pub const FIND: &str = "eludite.search.find";
pub const REPLACE: &str = "eludite.search.replace";
pub const CANCEL: &str = "eludite.search.cancel";
pub const RESULTS: &str = "eludite.search.results";

pub const ALL: [&str; 4] = [FIND, REPLACE, CANCEL, RESULTS];

/// `max_results`' default and cap.
pub const DEFAULT_MAX_RESULTS: usize = 1000;
pub const MAX_RESULTS: usize = 10_000;
/// The most context lines.
pub const MAX_CONTEXT: usize = 3;
/// `results`' default and largest page.
pub const DEFAULT_PAGE: usize = 200;
pub const MAX_PAGE: usize = 1000;
/// A line longer than this many characters is cut around its first match in answers.
pub const MAX_LINE_CHARS: usize = 1000;

/// (title, input schema, output schema, permission)
fn schemas(id: &str) -> (&'static str, &'static str, &'static str, PermissionClass) {
    use PermissionClass::*;
    macro_rules! s {
        ($name:literal) => {
            (
                include_str!(concat!(
                    "../../../protocol/schemas/search-",
                    $name,
                    ".input.json"
                )),
                include_str!(concat!(
                    "../../../protocol/schemas/search-",
                    $name,
                    ".output.json"
                )),
            )
        };
    }
    let (title, (input, output), class) = match id {
        FIND => ("Edit: Find in Files", s!("find"), Read),
        REPLACE => ("Edit: Replace in Files", s!("replace"), EditBuffer),
        CANCEL => ("Find Results: Stop Search", s!("cancel"), Read),
        RESULTS => ("Find Results: Results", s!("results"), Read),
        other => unreachable!("not a search command: {other}"),
    };
    (title, input, output, class)
}

/// Look in.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScopeKind {
    /// Entire Solution.
    #[default]
    Solution,
    /// Current Project.
    Project,
    /// Current Document.
    Document,
    /// All Open Documents.
    OpenDocuments,
    /// A folder.
    Folder,
}

impl ScopeKind {
    /// As the dialog's Look in list and the count line name it.
    pub fn label(self) -> &'static str {
        match self {
            ScopeKind::Solution => "Entire Solution",
            ScopeKind::Project => "Current Project",
            ScopeKind::Document => "Current Document",
            ScopeKind::OpenDocuments => "All Open Documents",
            ScopeKind::Folder => "Folder",
        }
    }
}

/// F8 and Shift+F8.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Navigate {
    Next,
    Previous,
}

/// What to find and where (the find half of both dialogs).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FindArgs {
    /// `None`: open the dialog (the person's key without a query).
    pub query: Option<String>,
    pub regex: bool,
    pub case_sensitive: bool,
    pub whole_word: bool,
    pub scope: ScopeKind,
    pub project: Option<String>,
    pub path: Option<String>,
    pub include: Vec<String>,
    pub exclude: Vec<String>,
    pub max_results: usize,
    pub context_lines: usize,
    pub results_window: Option<u8>,
    pub append: bool,
}

impl Default for FindArgs {
    fn default() -> Self {
        Self {
            query: None,
            regex: false,
            case_sensitive: false,
            whole_word: false,
            scope: ScopeKind::Solution,
            project: None,
            path: None,
            include: Vec::new(),
            exclude: Vec::new(),
            max_results: DEFAULT_MAX_RESULTS,
            context_lines: 0,
            results_window: None,
            append: false,
        }
    }
}

impl FindArgs {
    /// The File types box's text: the include globs, `;`-separated.
    pub fn file_types(&self) -> String {
        self.include.join(";")
    }
}

/// Split Visual Studio's File types text (`*.cs;*.cshtml;!*.g.cs`) into globs.
pub fn split_file_types(text: &str) -> Vec<String> {
    text.split(';')
        .map(str::trim)
        .filter(|g| !g.is_empty())
        .map(str::to_owned)
        .collect()
}

/// A parsed, validated search command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SearchRequest {
    Find(FindArgs),
    Replace {
        find: FindArgs,
        /// `None`: open the Replace in Files dialog.
        replacement: Option<String>,
        preview: bool,
        keep_open: bool,
    },
    Cancel {
        search_id: Option<u64>,
    },
    Results {
        results_window: Option<u8>,
        search_id: Option<u64>,
        offset: usize,
        limit: usize,
        navigate: Option<Navigate>,
    },
}

impl SearchRequest {
    pub fn command(&self) -> &'static str {
        match self {
            SearchRequest::Find(_) => FIND,
            SearchRequest::Replace { .. } => REPLACE,
            SearchRequest::Cancel { .. } => CANCEL,
            SearchRequest::Results { .. } => RESULTS,
        }
    }
}

// ----- Outputs -----

/// A match's place in its line's `text`, in characters.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct CharRange {
    pub start: usize,
    pub end: usize,
}

/// A matching line of `search-find.output.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MatchOut {
    pub line: u64,
    pub column: u64,
    pub text: String,
    pub ranges: Vec<CharRange>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub before: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub after: Vec<String>,
}

/// A matching file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileOut {
    pub path: String,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub open: bool,
    pub matches: Vec<MatchOut>,
}

/// `search-find.output.json`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FindOutput {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub search_id: Option<u64>,
    pub files: Vec<FileOut>,
    pub total: u64,
    pub matching_lines: u64,
    pub matching_files: u64,
    pub files_searched: u64,
    pub truncated: bool,
    pub canceled: bool,
    pub elapsed_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub results_window: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dialog: Option<bool>,
}

/// A file of `search-replace.output.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReplaceFile {
    pub path: String,
    pub replacements: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub change: Option<u64>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub open: bool,
}

/// A file Replace All left alone, and why.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Skipped {
    pub path: String,
    pub reason: String,
}

/// `search-replace.output.json`'s `state`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReplaceState {
    Pending,
    Applied,
    Failed,
    Nothing,
    Dialog,
}

/// `search-replace.output.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReplaceOutput {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub search_id: Option<u64>,
    pub state: ReplaceState,
    pub files: Vec<ReplaceFile>,
    pub replacements: u64,
    pub skipped: Vec<Skipped>,
    pub truncated: bool,
    pub elapsed_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

/// `search-cancel.output.json`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CancelOutput {
    pub canceled: Vec<u64>,
}

/// A block of a results window (one search).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlockOut {
    pub search_id: u64,
    pub summary: String,
    pub running: bool,
    pub canceled: bool,
    pub truncated: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub first: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total: Option<u64>,
}

/// A match of `search-results.output.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RowOut {
    pub path: String,
    pub line: u64,
    pub column: u64,
    pub text: String,
    pub ranges: Vec<CharRange>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub before: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub after: Vec<String>,
    pub block: u64,
    pub index: u64,
}

/// `search-results.output.json`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResultsOutput {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub results_window: Option<u8>,
    pub blocks: Vec<BlockOut>,
    pub matches: Vec<RowOut>,
    pub total: u64,
    pub offset: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_offset: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selected: Option<u64>,
}

/// A search command's answer.
#[derive(Debug, Clone, PartialEq)]
pub enum SearchOutput {
    Find(FindOutput),
    Replace(ReplaceOutput),
    Cancel(CancelOutput),
    Results(ResultsOutput),
}

impl SearchOutput {
    pub fn to_json(&self) -> Value {
        match self {
            SearchOutput::Find(o) => serde_json::to_value(o),
            SearchOutput::Replace(o) => serde_json::to_value(o),
            SearchOutput::Cancel(o) => serde_json::to_value(o),
            SearchOutput::Results(o) => serde_json::to_value(o),
        }
        .expect("search outputs serialize")
    }
}

/// Whatever searches (the shell). Called on the invoking thread.
pub trait SearchCommands: Send + Sync {
    fn apply(&self, request: SearchRequest) -> Result<SearchOutput, CommandError>;
}

/// A line's text for an answer: cut to [`MAX_LINE_CHARS`] around its first match (`…` marks a cut end), and the
/// byte ranges of the matches as character ranges in what is kept. Returns (text, ranges, the first match's column,
/// 1-based in characters of the whole line).
pub fn clip_line(text: &str, ranges: &[std::ops::Range<usize>]) -> (String, Vec<CharRange>, u64) {
    let chars_before = |b: usize| text.get(..b).map_or(0, |s| s.chars().count());
    let column = ranges
        .first()
        .map_or(1, |r| chars_before(r.start) as u64 + 1);
    let total = text.chars().count();
    let char_ranges: Vec<CharRange> = ranges
        .iter()
        .map(|r| CharRange {
            start: chars_before(r.start),
            end: chars_before(r.end),
        })
        .collect();
    if total <= MAX_LINE_CHARS {
        return (text.to_owned(), char_ranges, column);
    }
    // Keep a window starting a little before the first match.
    let first = char_ranges.first().map_or(0, |r| r.start);
    let start = first.saturating_sub(MAX_LINE_CHARS / 4);
    let end = (start + MAX_LINE_CHARS).min(total);
    let start = end.saturating_sub(MAX_LINE_CHARS);
    let mut kept: String = text.chars().skip(start).take(end - start).collect();
    let lead = usize::from(start > 0);
    if start > 0 {
        kept.insert(0, '\u{2026}');
    }
    if end < total {
        kept.push('\u{2026}');
    }
    let ranges = char_ranges
        .into_iter()
        .filter(|r| r.start >= start && r.end <= end)
        .map(|r| CharRange {
            start: r.start - start + lead,
            end: r.end - start + lead,
        })
        .collect();
    (kept, ranges, column)
}

// ----- Input -----

/// The input of `find` and `replace` (`replace` has three more members).
#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct FindIn {
    query: Option<String>,
    regex: Option<bool>,
    case_sensitive: Option<bool>,
    whole_word: Option<bool>,
    scope: Option<ScopeKind>,
    project: Option<String>,
    path: Option<String>,
    include: Option<Vec<String>>,
    exclude: Option<Vec<String>>,
    max_results: Option<u64>,
    context_lines: Option<u64>,
    results_window: Option<u8>,
    append: Option<bool>,
    replacement: Option<String>,
    preview: Option<bool>,
    keep_open: Option<bool>,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct CancelIn {
    search_id: Option<u64>,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct ResultsIn {
    results_window: Option<u8>,
    search_id: Option<u64>,
    offset: Option<u64>,
    limit: Option<u64>,
    navigate: Option<Navigate>,
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

fn window(name: &str, w: Option<u8>) -> Result<Option<u8>, CommandError> {
    match w {
        None | Some(1) | Some(2) => Ok(w),
        Some(_) => Err(invalid(format!(
            "`{name}` is 1 (Find Results 1) or 2 (Find Results 2)"
        ))),
    }
}

fn globs(name: &str, v: Option<Vec<String>>) -> Result<Vec<String>, CommandError> {
    let v = v.unwrap_or_default();
    if v.len() > 100 {
        return Err(invalid(format!("`{name}` has at most 100 globs")));
    }
    if v.iter().any(|g| g.trim().is_empty() || g.len() > 260) {
        return Err(invalid(format!("`{name}`'s globs are 1 to 260 characters")));
    }
    Ok(v)
}

fn find_args(i: FindIn) -> Result<FindArgs, CommandError> {
    if let Some(q) = &i.query {
        if q.is_empty() {
            return Err(invalid("`query` must not be empty"));
        }
        if q.chars().count() > 1000 {
            return Err(invalid("`query` is at most 1000 characters"));
        }
    }
    // `max_results` is capped, not refused: an agent asking for more gets the cap.
    let max_results = match i.max_results {
        None => DEFAULT_MAX_RESULTS,
        Some(0) => return Err(invalid("`max_results` is at least 1")),
        Some(n) => (n as usize).min(MAX_RESULTS),
    };
    let context_lines = match i.context_lines {
        None => 0,
        Some(n) if n as usize <= MAX_CONTEXT => n as usize,
        Some(_) => return Err(invalid(format!("`context_lines` is 0 to {MAX_CONTEXT}"))),
    };
    let scope = i.scope.unwrap_or_default();
    let path = match i.path {
        Some(p) if p.trim().is_empty() => return Err(invalid("`path` must not be empty")),
        other => other,
    };
    if scope == ScopeKind::Folder && path.is_none() {
        return Err(invalid("`scope: folder` needs the folder's `path`"));
    }
    let project = match i.project {
        Some(p) if p.trim().is_empty() => return Err(invalid("`project` must not be empty")),
        other => other,
    };
    Ok(FindArgs {
        query: i.query,
        regex: i.regex.unwrap_or(false),
        case_sensitive: i.case_sensitive.unwrap_or(false),
        whole_word: i.whole_word.unwrap_or(false),
        scope,
        project,
        path,
        include: globs("include", i.include)?,
        exclude: globs("exclude", i.exclude)?,
        max_results,
        context_lines,
        results_window: window("results_window", i.results_window)?,
        append: i.append.unwrap_or(false),
    })
}

/// Parse and validate the input of search command `id`.
pub fn parse(id: &str, value: Value) -> Result<SearchRequest, CommandError> {
    Ok(match id {
        FIND => {
            let i: FindIn = input(value)?;
            for (name, given) in [
                ("replacement", i.replacement.is_some()),
                ("preview", i.preview.is_some()),
                ("keep_open", i.keep_open.is_some()),
            ] {
                if given {
                    return Err(invalid(format!(
                        "unknown field `{name}`: it is eludite.search.replace's"
                    )));
                }
            }
            SearchRequest::Find(find_args(i)?)
        }
        REPLACE => {
            let mut i: FindIn = input(value)?;
            if i.replacement.as_ref().is_some_and(|r| r.len() > 10_000) {
                return Err(invalid("`replacement` is at most 10000 bytes"));
            }
            let (replacement, preview, keep_open) =
                (i.replacement.take(), i.preview.take(), i.keep_open.take());
            let find = find_args(i)?;
            if find.query.is_some() && replacement.is_none() {
                return Err(invalid(
                    "`replacement` is required with a `query` (an empty string deletes the matches)",
                ));
            }
            SearchRequest::Replace {
                find,
                replacement,
                preview: preview.unwrap_or(true),
                keep_open: keep_open.unwrap_or(true),
            }
        }
        CANCEL => {
            let i: CancelIn = input(value)?;
            SearchRequest::Cancel {
                search_id: i.search_id,
            }
        }
        RESULTS => {
            let i: ResultsIn = input(value)?;
            let limit = match i.limit {
                None => DEFAULT_PAGE,
                Some(n) if (1..=MAX_PAGE as u64).contains(&n) => n as usize,
                Some(_) => return Err(invalid(format!("`limit` is 1 to {MAX_PAGE}"))),
            };
            if i.search_id.is_some() && i.results_window.is_some() {
                return Err(invalid("give `results_window` or `search_id`, not both"));
            }
            if i.search_id.is_some() && i.navigate.is_some() {
                return Err(invalid(
                    "`navigate` steps through a results window, not a search",
                ));
            }
            SearchRequest::Results {
                results_window: window("results_window", i.results_window)?,
                search_id: i.search_id,
                offset: i.offset.unwrap_or(0) as usize,
                limit,
                navigate: i.navigate,
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

/// The escalation hook of search command `id`: `replace` with `preview: false` writes at once, so it is execute.
pub fn escalation(id: &'static str) -> Option<EscalationHook> {
    if id != REPLACE {
        return None;
    }
    Some(Arc::new(|input: &Value, _: &PolicyView| {
        (input.get("preview").and_then(Value::as_bool) == Some(false)).then(|| {
            Escalation::raise(
                PermissionClass::Execute,
                "eludite.search.replace with `preview: false` writes the replacements at once, without review",
            )
        })
    }))
}

/// Register every search command, applying them to `target`.
pub fn register(registry: &CommandRegistry, target: Arc<dyn SearchCommands>) {
    for id in ALL {
        let target = target.clone();
        registry.replace_with_escalation(spec(id), escalation(id), move |input| {
            let request = parse(id, input)?;
            target.apply(request).map(|out| out.to_json())
        });
    }
}

#[cfg(test)]
#[allow(clippy::single_range_in_vec_init)]
mod tests {
    use super::*;
    use crate::policy::{AgentPolicy, Verdict};
    use crate::{Caller, with_caller};
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
                let a = value.as_array().unwrap();
                if let Some(max) = schema["maxItems"].as_u64() {
                    assert!(a.len() as u64 <= max, "{at}: more than {max}");
                }
                for (i, v) in a.iter().enumerate() {
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
                REPLACE => PermissionClass::EditBuffer,
                _ => PermissionClass::Read,
            };
            assert_eq!(s.permission, want, "{id}");
        }
        // The replace schema is the find schema and three more members.
        let find = spec(FIND).input_schema["properties"].clone();
        let replace = spec(REPLACE).input_schema["properties"].clone();
        for k in find.as_object().unwrap().keys() {
            assert!(replace.get(k).is_some(), "replace lacks {k}");
        }
        for k in ["replacement", "preview", "keep_open"] {
            assert!(replace.get(k).is_some(), "{k}");
        }
    }

    #[test]
    fn parses_validates_and_caps_input() {
        let SearchRequest::Find(a) = parse(FIND, json!({"query": "Order"})).unwrap() else {
            panic!()
        };
        assert_eq!(
            a,
            FindArgs {
                query: Some("Order".into()),
                ..FindArgs::default()
            }
        );
        // No query: the dialog.
        assert!(matches!(
            parse(FIND, Value::Null).unwrap(),
            SearchRequest::Find(FindArgs { query: None, .. })
        ));
        // max_results is capped at 10,000, not refused.
        let SearchRequest::Find(a) =
            parse(FIND, json!({"query": "x", "max_results": 50_000})).unwrap()
        else {
            panic!()
        };
        assert_eq!(a.max_results, MAX_RESULTS);
        assert!(parse(FIND, json!({"query": "x", "max_results": 0})).is_err());
        assert!(parse(FIND, json!({"query": ""})).is_err());
        assert!(parse(FIND, json!({"query": "x", "context_lines": 4})).is_err());
        assert!(parse(FIND, json!({"query": "x", "results_window": 3})).is_err());
        assert!(parse(FIND, json!({"query": "x", "scope": "folder"})).is_err());
        assert!(parse(FIND, json!({"query": "x", "scope": "everywhere"})).is_err());
        assert!(parse(FIND, json!({"query": "x", "include": [" "]})).is_err());
        assert!(parse(FIND, json!({"query": "x", "bogus": 1})).is_err());
        let SearchRequest::Find(a) = parse(
            FIND,
            json!({"query": "x", "scope": "open_documents", "include": ["*.cs", "!*.g.cs"],
                   "regex": true, "whole_word": true, "case_sensitive": true, "results_window": 2, "append": true}),
        )
        .unwrap() else {
            panic!()
        };
        assert_eq!(a.scope, ScopeKind::OpenDocuments);
        assert_eq!(a.file_types(), "*.cs;!*.g.cs");
        assert!(a.regex && a.whole_word && a.case_sensitive && a.append);
        assert_eq!(a.results_window, Some(2));
        // Replace: the find members and three more; preview and keep_open default on.
        let r = parse(REPLACE, json!({"query": "a", "replacement": "b"})).unwrap();
        assert!(matches!(
            r,
            SearchRequest::Replace {
                preview: true,
                keep_open: true,
                ..
            }
        ));
        assert!(
            parse(REPLACE, json!({"query": "a"})).is_err(),
            "a query needs a replacement"
        );
        assert!(
            parse(
                REPLACE,
                json!({"query": "a", "replacement": "b", "bogus": 1})
            )
            .is_err()
        );
        assert!(matches!(
            parse(REPLACE, json!({})).unwrap(),
            SearchRequest::Replace {
                replacement: None,
                ..
            }
        ));
        assert_eq!(
            parse(RESULTS, json!({"navigate": "next"})).unwrap(),
            SearchRequest::Results {
                results_window: None,
                search_id: None,
                offset: 0,
                limit: DEFAULT_PAGE,
                navigate: Some(Navigate::Next)
            }
        );
        assert!(parse(RESULTS, json!({"limit": 1001})).is_err());
        assert!(parse(RESULTS, json!({"search_id": 1, "results_window": 1})).is_err());
        assert!(parse(RESULTS, json!({"search_id": 1, "navigate": "next"})).is_err());
        assert_eq!(
            parse(CANCEL, json!({})).unwrap(),
            SearchRequest::Cancel { search_id: None }
        );
        assert!(parse("eludite.search.nope", json!({})).is_err());
        assert_eq!(
            split_file_types(" *.cs; *.cshtml ;;!*.g.cs"),
            ["*.cs", "*.cshtml", "!*.g.cs"]
        );
    }

    #[test]
    fn long_lines_are_cut_around_their_first_match() {
        let (t, r, c) = clip_line("héllo world", &[7..12]);
        assert_eq!(t, "héllo world");
        assert_eq!(r, [CharRange { start: 6, end: 11 }]);
        assert_eq!(c, 7);
        let long = format!("{}needle{}", "a".repeat(5000), "b".repeat(5000));
        let (t, r, c) = clip_line(&long, &[5000..5006]);
        assert_eq!(c, 5001);
        assert!(t.starts_with('\u{2026}') && t.ends_with('\u{2026}'));
        assert!(t.chars().count() <= MAX_LINE_CHARS + 2);
        let kept: String = t
            .chars()
            .skip(r[0].start)
            .take(r[0].end - r[0].start)
            .collect();
        assert_eq!(kept, "needle");
    }

    #[test]
    fn outputs_follow_their_schemas() {
        let m = MatchOut {
            line: 3,
            column: 5,
            text: "var order = 1;".into(),
            ranges: vec![CharRange { start: 4, end: 9 }],
            before: vec!["{".into()],
            after: vec![],
        };
        let cases: Vec<(&str, SearchOutput)> = vec![
            (
                FIND,
                SearchOutput::Find(FindOutput {
                    search_id: Some(4),
                    files: vec![FileOut {
                        path: "src/A.cs".into(),
                        open: true,
                        matches: vec![m.clone()],
                    }],
                    total: 1,
                    matching_lines: 1,
                    matching_files: 1,
                    files_searched: 9,
                    truncated: false,
                    canceled: false,
                    elapsed_ms: 12,
                    results_window: Some(1),
                    dialog: None,
                }),
            ),
            (
                FIND,
                SearchOutput::Find(FindOutput {
                    dialog: Some(true),
                    ..Default::default()
                }),
            ),
            (
                REPLACE,
                SearchOutput::Replace(ReplaceOutput {
                    search_id: Some(5),
                    state: ReplaceState::Pending,
                    files: vec![ReplaceFile {
                        path: "src/A.cs".into(),
                        replacements: 2,
                        change: Some(7),
                        open: true,
                    }],
                    replacements: 2,
                    skipped: vec![Skipped {
                        path: "src/B.cs".into(),
                        reason: "changed since the search".into(),
                    }],
                    truncated: false,
                    elapsed_ms: 3,
                    message: Some("2 replacements in 1 file held for review".into()),
                }),
            ),
            (
                CANCEL,
                SearchOutput::Cancel(CancelOutput { canceled: vec![4] }),
            ),
            (
                RESULTS,
                SearchOutput::Results(ResultsOutput {
                    results_window: Some(1),
                    blocks: vec![BlockOut {
                        search_id: 4,
                        summary: "Find all \"order\", Subfolders, Find Results 1, Entire Solution"
                            .into(),
                        running: false,
                        canceled: false,
                        truncated: false,
                        first: Some(0),
                        total: Some(1),
                    }],
                    matches: vec![RowOut {
                        path: "src/A.cs".into(),
                        line: 3,
                        column: 5,
                        text: m.text.clone(),
                        ranges: m.ranges.clone(),
                        before: vec![],
                        after: vec!["}".into()],
                        block: 0,
                        index: 0,
                    }],
                    total: 1,
                    offset: 0,
                    next_offset: None,
                    selected: Some(0),
                }),
            ),
        ];
        for (id, out) in cases {
            let (_, _, schema, _) = schemas(id);
            conforms(schema, &out.to_json());
        }
    }

    struct Fake;
    impl SearchCommands for Fake {
        fn apply(&self, request: SearchRequest) -> Result<SearchOutput, CommandError> {
            Ok(match request {
                SearchRequest::Find(a) => SearchOutput::Find(FindOutput {
                    total: a.max_results as u64,
                    ..Default::default()
                }),
                _ => SearchOutput::Cancel(CancelOutput::default()),
            })
        }
    }

    #[test]
    fn the_classes_apply_through_the_registry() {
        let registry = CommandRegistry::new();
        register(&registry, Arc::new(Fake));
        let class = |id: &str, input: Value| registry.classify(id, &input).unwrap().class;
        assert_eq!(class(FIND, json!({"query": "x"})), PermissionClass::Read);
        assert_eq!(class(CANCEL, json!({})), PermissionClass::Read);
        assert_eq!(class(RESULTS, json!({})), PermissionClass::Read);
        // Replace with preview is held for review; without, it is execute.
        let preview = json!({"query": "a", "replacement": "b"});
        assert_eq!(class(REPLACE, preview.clone()), PermissionClass::EditBuffer);
        let p = AgentPolicy::default();
        let held = registry.classify(REPLACE, &preview).unwrap();
        assert_eq!(
            p.decide_call(&held, "eludite-search-replace", &preview),
            Verdict::Review
        );
        let now = json!({"query": "a", "replacement": "b", "preview": false});
        let call = registry.classify(REPLACE, &now).unwrap();
        assert_eq!(call.class, PermissionClass::Execute);
        assert!(call.reason.as_deref().unwrap().contains("preview: false"));
        assert_eq!(
            p.decide_call(&call, "eludite-search-replace", &now),
            Verdict::Ask
        );
        // The cap reaches the target.
        let out = with_caller(Caller::User, || {
            registry.invoke(FIND, json!({"query": "x", "max_results": 99_999}))
        })
        .unwrap();
        assert_eq!(out["total"], MAX_RESULTS as u64);
    }
}
