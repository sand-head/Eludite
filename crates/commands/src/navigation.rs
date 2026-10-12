//! The navigation set (brief 0071): `eludite.navigation.*`, what Visual Studio's Edit > Go To items, the Document
//! Outline, the navigation bar, Call Hierarchy, Type Hierarchy, reference highlighting and the Find All References
//! toolbar do, for the person and agents alike.
//!
//! - **Go To**: `go_to_implementation` (Ctrl+F12), `go_to_type_definition`, `go_to_base` (Alt+Home) and
//!   `go_to_derived` (Alt+End), with the picker Go To Definition has when there are several targets.
//! - **Go To All**: `go_to_all` (Ctrl+T), files from the workspace's tree and symbols from every language server, with
//!   Visual Studio's filter prefixes ([`Filter::from_query`]).
//! - **Reference highlighting**: `highlight_references`, and stepping between the highlights (Ctrl+Shift+Down, Up).
//! - **Outline**: `document_outline` (the Document Outline window), `file_structure` (Rider's popup, VS's Go To
//!   Member), `navigation_bar` (the three dropdowns above an editor).
//! - **Hierarchies**: `call_hierarchy` and `type_hierarchy`, expanded lazily.
//! - **Find All References**: `references`, its Kind filter, Group By, search, Keep Results and F8 / Shift+F8.
//!
//! The schemas are `protocol/schemas/navigation-*.json` (checked in first, CLAUDE.md invariant 4). This module parses
//! input into a typed [`NavigationRequest`] and serializes the typed [`NavigationOutput`]; the shell implements
//! [`NavigationCommands`] on its UI thread. Every command is class `read` and visible to agents.

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{CommandError, CommandId, CommandRegistry, CommandSpec, PermissionClass};

pub const GO_TO_IMPLEMENTATION: &str = "eludite.navigation.go_to_implementation";
pub const GO_TO_TYPE_DEFINITION: &str = "eludite.navigation.go_to_type_definition";
pub const GO_TO_BASE: &str = "eludite.navigation.go_to_base";
pub const GO_TO_DERIVED: &str = "eludite.navigation.go_to_derived";
pub const GO_TO_ALL: &str = "eludite.navigation.go_to_all";
pub const HIGHLIGHT_REFERENCES: &str = "eludite.navigation.highlight_references";
pub const DOCUMENT_OUTLINE: &str = "eludite.navigation.document_outline";
pub const FILE_STRUCTURE: &str = "eludite.navigation.file_structure";
pub const NAVIGATION_BAR: &str = "eludite.navigation.navigation_bar";
pub const CALL_HIERARCHY: &str = "eludite.navigation.call_hierarchy";
pub const TYPE_HIERARCHY: &str = "eludite.navigation.type_hierarchy";
pub const REFERENCES: &str = "eludite.navigation.references";

/// Every command this module registers.
pub const ALL: [&str; 12] = [
    GO_TO_IMPLEMENTATION,
    GO_TO_TYPE_DEFINITION,
    GO_TO_BASE,
    GO_TO_DERIVED,
    GO_TO_ALL,
    HIGHLIGHT_REFERENCES,
    DOCUMENT_OUTLINE,
    FILE_STRUCTURE,
    NAVIGATION_BAR,
    CALL_HIERARCHY,
    TYPE_HIERARCHY,
    REFERENCES,
];

/// Go To All's default and largest result count.
pub const DEFAULT_RESULTS: usize = 100;
pub const MAX_RESULTS: usize = 500;
/// Targets and references an answer lists at most.
pub const MAX_TARGETS: usize = 100;
pub const MAX_REFERENCES: usize = 1000;

/// (title, input schema, output schema)
fn schemas(id: &str) -> (&'static str, &'static str, &'static str) {
    macro_rules! s {
        ($input:literal, $output:literal) => {
            (
                include_str!(concat!(
                    "../../../protocol/schemas/navigation-",
                    $input,
                    ".input.json"
                )),
                include_str!(concat!(
                    "../../../protocol/schemas/navigation-",
                    $output,
                    ".output.json"
                )),
            )
        };
    }
    let (title, (input, output)) = match id {
        GO_TO_IMPLEMENTATION => ("Edit: Go To Implementation", s!("go-to", "go-to")),
        GO_TO_TYPE_DEFINITION => ("Edit: Go To Type Definition", s!("go-to", "go-to")),
        GO_TO_BASE => ("Edit: Go To Base", s!("go-to", "go-to")),
        GO_TO_DERIVED => ("Edit: Go To Derived Symbols", s!("go-to", "go-to")),
        GO_TO_ALL => ("Edit: Go To All", s!("go-to-all", "go-to-all")),
        HIGHLIGHT_REFERENCES => (
            "Edit: Highlight References",
            s!("highlight-references", "highlight-references"),
        ),
        DOCUMENT_OUTLINE => (
            "View: Document Outline",
            s!("document-outline", "document-outline"),
        ),
        FILE_STRUCTURE => (
            "Edit: Go To Member (File Structure)",
            s!("file-structure", "file-structure"),
        ),
        NAVIGATION_BAR => (
            "Editor: Navigation Bar",
            s!("navigation-bar", "navigation-bar"),
        ),
        CALL_HIERARCHY => (
            "View: Call Hierarchy",
            s!("call-hierarchy", "call-hierarchy"),
        ),
        TYPE_HIERARCHY => (
            "View: Type Hierarchy",
            s!("type-hierarchy", "type-hierarchy"),
        ),
        REFERENCES => ("Find All References: View", s!("references", "references")),
        other => unreachable!("not a navigation command: {other}"),
    };
    (title, input, output)
}

/// The public description of navigation command `id`.
pub fn spec(id: &str) -> CommandSpec {
    let (title, input, output) = schemas(id);
    CommandSpec {
        id: CommandId::new(id).expect("valid id"),
        title: title.into(),
        input_schema: serde_json::from_str(input).expect("protocol schemas are valid JSON"),
        output_schema: serde_json::from_str(output).expect("protocol schemas are valid JSON"),
        permission: PermissionClass::Read,
        agent_visible: true,
    }
}

// ----- Requests -----

/// Which Go To command.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GoToKind {
    Implementation,
    TypeDefinition,
    Base,
    Derived,
}

impl GoToKind {
    pub fn command(self) -> &'static str {
        match self {
            GoToKind::Implementation => GO_TO_IMPLEMENTATION,
            GoToKind::TypeDefinition => GO_TO_TYPE_DEFINITION,
            GoToKind::Base => GO_TO_BASE,
            GoToKind::Derived => GO_TO_DERIVED,
        }
    }

    /// The LSP request that answers it (Go To Base prepares a type hierarchy first).
    pub fn method(self) -> &'static str {
        match self {
            GoToKind::Implementation | GoToKind::Derived => "textDocument/implementation",
            GoToKind::TypeDefinition => "textDocument/typeDefinition",
            GoToKind::Base => "textDocument/prepareTypeHierarchy",
        }
    }

    /// Visual Studio's name, for the picker's title and messages.
    pub fn label(self) -> &'static str {
        match self {
            GoToKind::Implementation => "implementations",
            GoToKind::TypeDefinition => "type definitions",
            GoToKind::Base => "base symbols",
            GoToKind::Derived => "derived symbols",
        }
    }
}

/// Go To All's filter (Visual Studio's prefixes and Ctrl+1 chords).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Filter {
    #[default]
    All,
    Files,
    Types,
    Members,
    Symbols,
    Line,
}

impl Filter {
    /// The filter a query's prefix names (`f `, `t `, `m `, `# `, `:`), and the text after it.
    pub fn from_query(query: &str) -> (Option<Filter>, &str) {
        if let Some(rest) = query.strip_prefix(':') {
            return (Some(Filter::Line), rest.trim_start());
        }
        for (prefix, filter) in [
            ("f ", Filter::Files),
            ("t ", Filter::Types),
            ("m ", Filter::Members),
            ("# ", Filter::Symbols),
        ] {
            if let Some(rest) = query.strip_prefix(prefix) {
                return (Some(filter), rest.trim_start());
            }
        }
        (None, query)
    }

    /// The prefix that selects it (`""` for all).
    pub fn prefix(self) -> &'static str {
        match self {
            Filter::All => "",
            Filter::Files => "f ",
            Filter::Types => "t ",
            Filter::Members => "m ",
            Filter::Symbols => "# ",
            Filter::Line => ":",
        }
    }

    pub fn wants_files(self) -> bool {
        matches!(self, Filter::All | Filter::Files)
    }

    pub fn wants_symbols(self) -> bool {
        matches!(
            self,
            Filter::All | Filter::Types | Filter::Members | Filter::Symbols
        )
    }

    /// Whether a symbol of LSP kind `kind` passes the filter.
    pub fn keeps_symbol(self, kind: u32) -> bool {
        match self {
            Filter::Types => matches!(kind, 5 | 10 | 11 | 23 | 26),
            Filter::Members => matches!(kind, 6 | 7 | 8 | 9 | 12 | 14 | 22 | 24 | 25),
            Filter::All | Filter::Symbols => true,
            Filter::Files | Filter::Line => false,
        }
    }
}

/// `next` or `previous` (F8 and Shift+F8, Ctrl+Shift+Down and Up).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Step {
    Next,
    Previous,
}

/// The navigation bar's dropdowns that can be chosen from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Dropdown {
    Types,
    Members,
}

/// A Call Hierarchy folder.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CallDirection {
    #[default]
    CallsTo,
    CallsFrom,
}

/// A Type Hierarchy direction.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TypeDirection {
    #[default]
    BaseTypes,
    DerivedTypes,
}

/// Visual Studio's Kind column of Find All References.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReferenceKind {
    Definition,
    Read,
    Write,
    Reference,
}

impl ReferenceKind {
    pub const ALL: [ReferenceKind; 4] = [
        ReferenceKind::Definition,
        ReferenceKind::Read,
        ReferenceKind::Write,
        ReferenceKind::Reference,
    ];

    /// As the Kind column shows it.
    pub fn label(self) -> &'static str {
        match self {
            ReferenceKind::Definition => "Definition",
            ReferenceKind::Read => "Read",
            ReferenceKind::Write => "Write",
            ReferenceKind::Reference => "Reference",
        }
    }
}

/// Find All References' Group By.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GroupBy {
    /// Project then File (Visual Studio's default).
    #[default]
    ProjectFile,
    File,
    Definition,
    None,
}

impl GroupBy {
    pub const ALL: [GroupBy; 4] = [
        GroupBy::ProjectFile,
        GroupBy::File,
        GroupBy::Definition,
        GroupBy::None,
    ];

    /// As the Group By list shows it.
    pub fn label(self) -> &'static str {
        match self {
            GroupBy::ProjectFile => "Project then File",
            GroupBy::File => "File",
            GroupBy::Definition => "Definition",
            GroupBy::None => "None",
        }
    }
}

/// A 1-based position in a document (`None` members: the active document, the caret).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct At {
    pub path: Option<String>,
    pub line: Option<u32>,
    pub column: Option<u32>,
}

impl At {
    /// The position as the shell's caret: `None` for the caret, else `(line, column)` with column 1 by default.
    pub fn caret(&self) -> Option<(u32, u32)> {
        self.line.map(|l| (l, self.column.unwrap_or(1)))
    }
}

/// A parsed, validated navigation command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NavigationRequest {
    GoTo {
        kind: GoToKind,
        at: At,
        target: Option<usize>,
    },
    GoToAll {
        query: Option<String>,
        filter: Option<Filter>,
        max_results: usize,
        select: Option<usize>,
        open_popup: bool,
    },
    Highlights {
        at: At,
        navigate: Option<Step>,
    },
    Outline {
        path: Option<String>,
        show: bool,
        filter: Option<String>,
    },
    FileStructure {
        path: Option<String>,
        filter: Option<String>,
        select: Option<usize>,
        close: bool,
    },
    NavigationBar {
        path: Option<String>,
        select: Option<(Dropdown, usize)>,
    },
    CallHierarchy {
        at: At,
        expand: Option<(u64, CallDirection)>,
        navigate: Option<(u64, Option<usize>)>,
    },
    TypeHierarchy {
        at: At,
        expand: Option<(u64, TypeDirection)>,
        navigate: Option<u64>,
    },
    References {
        kinds: Option<Vec<ReferenceKind>>,
        group_by: Option<GroupBy>,
        search: Option<String>,
        keep: Option<bool>,
        navigate: Option<Step>,
    },
}

impl NavigationRequest {
    pub fn command(&self) -> &'static str {
        match self {
            NavigationRequest::GoTo { kind, .. } => kind.command(),
            NavigationRequest::GoToAll { .. } => GO_TO_ALL,
            NavigationRequest::Highlights { .. } => HIGHLIGHT_REFERENCES,
            NavigationRequest::Outline { .. } => DOCUMENT_OUTLINE,
            NavigationRequest::FileStructure { .. } => FILE_STRUCTURE,
            NavigationRequest::NavigationBar { .. } => NAVIGATION_BAR,
            NavigationRequest::CallHierarchy { .. } => CALL_HIERARCHY,
            NavigationRequest::TypeHierarchy { .. } => TYPE_HIERARCHY,
            NavigationRequest::References { .. } => REFERENCES,
        }
    }
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct GoToIn {
    path: Option<String>,
    line: Option<u32>,
    column: Option<u32>,
    target: Option<usize>,
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct GoToAllIn {
    query: Option<String>,
    filter: Option<Filter>,
    max_results: Option<usize>,
    select: Option<usize>,
    open_popup: Option<bool>,
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct HighlightsIn {
    path: Option<String>,
    line: Option<u32>,
    column: Option<u32>,
    navigate: Option<Step>,
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct OutlineIn {
    path: Option<String>,
    show: Option<bool>,
    filter: Option<String>,
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct FileStructureIn {
    path: Option<String>,
    filter: Option<String>,
    select: Option<usize>,
    close: Option<bool>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SelectIn {
    dropdown: Dropdown,
    index: usize,
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct BarIn {
    path: Option<String>,
    select: Option<SelectIn>,
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct CallIn {
    path: Option<String>,
    line: Option<u32>,
    column: Option<u32>,
    expand: Option<u64>,
    direction: Option<CallDirection>,
    navigate: Option<u64>,
    site: Option<usize>,
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct TypeIn {
    path: Option<String>,
    line: Option<u32>,
    column: Option<u32>,
    expand: Option<u64>,
    direction: Option<TypeDirection>,
    navigate: Option<u64>,
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReferencesIn {
    kinds: Option<Vec<ReferenceKind>>,
    group_by: Option<GroupBy>,
    search: Option<String>,
    keep: Option<bool>,
    navigate: Option<Step>,
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

fn path(p: Option<String>) -> Result<Option<String>, CommandError> {
    match p {
        Some(p) if p.is_empty() => Err(invalid("`path` must not be empty")),
        other => Ok(other),
    }
}

fn at(p: Option<String>, line: Option<u32>, column: Option<u32>) -> Result<At, CommandError> {
    if line == Some(0) || column == Some(0) {
        return Err(invalid("`line` and `column` are 1-based"));
    }
    if column.is_some() && line.is_none() {
        return Err(invalid("`column` needs `line`"));
    }
    Ok(At {
        path: path(p)?,
        line,
        column,
    })
}

/// Parse and validate the input of navigation command `id`.
pub fn parse(id: &str, value: Value) -> Result<NavigationRequest, CommandError> {
    let go_to = |kind| -> Result<NavigationRequest, CommandError> {
        let i: GoToIn = input(value.clone())?;
        if i.target.is_some() && (i.line.is_some() || i.column.is_some() || i.path.is_some()) {
            return Err(invalid(
                "`target` chooses from the open picker; it takes no position",
            ));
        }
        Ok(NavigationRequest::GoTo {
            kind,
            at: at(i.path, i.line, i.column)?,
            target: i.target,
        })
    };
    Ok(match id {
        GO_TO_IMPLEMENTATION => go_to(GoToKind::Implementation)?,
        GO_TO_TYPE_DEFINITION => go_to(GoToKind::TypeDefinition)?,
        GO_TO_BASE => go_to(GoToKind::Base)?,
        GO_TO_DERIVED => go_to(GoToKind::Derived)?,
        GO_TO_ALL => {
            let i: GoToAllIn = input(value)?;
            let max_results = i.max_results.unwrap_or(DEFAULT_RESULTS);
            if max_results == 0 || max_results > MAX_RESULTS {
                return Err(invalid(format!("`max_results` must be 1 to {MAX_RESULTS}")));
            }
            if i.query.as_ref().is_some_and(|q| q.chars().count() > 1000) {
                return Err(invalid("`query` is at most 1000 characters"));
            }
            NavigationRequest::GoToAll {
                query: i.query,
                filter: i.filter,
                max_results,
                select: i.select,
                open_popup: i.open_popup.unwrap_or(false),
            }
        }
        HIGHLIGHT_REFERENCES => {
            let i: HighlightsIn = input(value)?;
            NavigationRequest::Highlights {
                at: at(i.path, i.line, i.column)?,
                navigate: i.navigate,
            }
        }
        DOCUMENT_OUTLINE => {
            let i: OutlineIn = input(value)?;
            NavigationRequest::Outline {
                path: path(i.path)?,
                show: i.show.unwrap_or(false),
                filter: i.filter,
            }
        }
        FILE_STRUCTURE => {
            let i: FileStructureIn = input(value)?;
            if i.close == Some(true) && i.select.is_some() {
                return Err(invalid("`select` and `close` exclude each other"));
            }
            NavigationRequest::FileStructure {
                path: path(i.path)?,
                filter: i.filter,
                select: i.select,
                close: i.close.unwrap_or(false),
            }
        }
        NAVIGATION_BAR => {
            let i: BarIn = input(value)?;
            NavigationRequest::NavigationBar {
                path: path(i.path)?,
                select: i.select.map(|s| (s.dropdown, s.index)),
            }
        }
        CALL_HIERARCHY => {
            let i: CallIn = input(value)?;
            if i.expand.is_some() && i.navigate.is_some() {
                return Err(invalid("`expand` and `navigate` exclude each other"));
            }
            if i.site.is_some() && i.navigate.is_none() {
                return Err(invalid("`site` needs `navigate`"));
            }
            if i.direction.is_some() && i.expand.is_none() {
                return Err(invalid("`direction` needs `expand`"));
            }
            NavigationRequest::CallHierarchy {
                at: at(i.path, i.line, i.column)?,
                expand: i.expand.map(|n| (n, i.direction.unwrap_or_default())),
                navigate: i.navigate.map(|n| (n, i.site)),
            }
        }
        TYPE_HIERARCHY => {
            let i: TypeIn = input(value)?;
            if i.expand.is_some() && i.navigate.is_some() {
                return Err(invalid("`expand` and `navigate` exclude each other"));
            }
            if i.direction.is_some() && i.expand.is_none() {
                return Err(invalid("`direction` needs `expand`"));
            }
            NavigationRequest::TypeHierarchy {
                at: at(i.path, i.line, i.column)?,
                expand: i.expand.map(|n| (n, i.direction.unwrap_or_default())),
                navigate: i.navigate,
            }
        }
        REFERENCES => {
            let i: ReferencesIn = input(value)?;
            NavigationRequest::References {
                kinds: i.kinds,
                group_by: i.group_by,
                search: i.search,
                keep: i.keep,
                navigate: i.navigate,
            }
        }
        other => return Err(invalid(format!("not a navigation command: {other}"))),
    })
}

// ----- Outputs -----

/// `state` of the outputs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum State {
    Loading,
    Navigated,
    Choose,
    Done,
    None,
    Off,
    Failed,
}

/// A Go To target.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GoToTarget {
    pub path: String,
    pub line: u32,
    pub column: u32,
    pub metadata: bool,
    pub generated: bool,
    pub title: String,
}

/// `navigation-go-to.output.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GoToOutput {
    pub kind: GoToKind,
    pub path: String,
    pub line: u32,
    pub column: u32,
    pub state: State,
    pub targets: Vec<GoToTarget>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub navigated: Option<GoToTarget>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

/// A row of `navigation-go-to-all.output.json`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GoToAllResult {
    pub name: String,
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub container: Option<String>,
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub column: Option<u32>,
    pub score: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server: Option<String>,
}

/// `navigation-go-to-all.output.json`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GoToAllOutput {
    pub query: String,
    pub filter: Filter,
    pub state: State,
    pub results: Vec<GoToAllResult>,
    pub total: usize,
    pub servers_pending: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

/// A highlight's kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HighlightKind {
    Text,
    Read,
    Write,
}

/// A range of `navigation-highlight-references.output.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HighlightOut {
    pub line: u32,
    pub column: u32,
    pub end_line: u32,
    pub end_column: u32,
    pub kind: HighlightKind,
}

/// `navigation-highlight-references.output.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HighlightsOutput {
    pub path: String,
    pub state: State,
    pub highlights: Vec<HighlightOut>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

/// A symbol of the outline outputs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SymbolOut {
    pub name: String,
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    pub line: u32,
    pub column: u32,
    pub end_line: u32,
    pub children: Vec<SymbolOut>,
}

/// `navigation-document-outline.output.json` and, with `open`, `navigation-file-structure.output.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OutlineOutput {
    pub path: String,
    pub state: State,
    pub symbols: Vec<SymbolOut>,
    pub current: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filter: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub open: Option<bool>,
}

/// An entry of the navigation bar's dropdowns.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BarEntry {
    pub name: String,
    pub kind: String,
    pub line: u32,
    pub column: u32,
}

/// `navigation-navigation-bar.output.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NavigationBarOutput {
    pub path: String,
    pub visible: bool,
    pub project: String,
    pub types: Vec<BarEntry>,
    pub members: Vec<BarEntry>,
    #[serde(default, rename = "type", skip_serializing_if = "Option::is_none")]
    pub type_index: Option<usize>,
    #[serde(default, rename = "member", skip_serializing_if = "Option::is_none")]
    pub member_index: Option<usize>,
    pub state: State,
}

/// A call site.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SiteOut {
    pub path: String,
    pub line: u32,
    pub column: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
}

/// A node of `navigation-call-hierarchy.output.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CallNodeOut {
    pub id: u64,
    pub name: String,
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    pub path: String,
    pub line: u32,
    pub column: u32,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sites: Vec<SiteOut>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub calls_to: Option<Vec<CallNodeOut>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub calls_from: Option<Vec<CallNodeOut>>,
}

/// `navigation-call-hierarchy.output.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CallHierarchyOutput {
    pub state: State,
    pub roots: Vec<CallNodeOut>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub navigated: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

/// A node of `navigation-type-hierarchy.output.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TypeNodeOut {
    pub id: u64,
    pub name: String,
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    pub path: String,
    pub line: u32,
    pub column: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_types: Option<Vec<TypeNodeOut>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub derived_types: Option<Vec<TypeNodeOut>>,
}

/// `navigation-type-hierarchy.output.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TypeHierarchyOutput {
    pub state: State,
    pub roots: Vec<TypeNodeOut>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub navigated: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

/// A reference of `navigation-references.output.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReferenceOut {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    pub path: String,
    pub line: u32,
    pub column: u32,
    pub text: String,
    pub kind: ReferenceKind,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub kept: bool,
}

/// `navigation-references.output.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReferencesViewOutput {
    pub symbol: String,
    pub kinds: Vec<ReferenceKind>,
    pub group_by: GroupBy,
    pub search: String,
    pub keep: bool,
    pub shown: usize,
    pub total: usize,
    pub references: Vec<ReferenceOut>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selected: Option<usize>,
}

/// A navigation command's answer.
#[derive(Debug, Clone, PartialEq)]
pub enum NavigationOutput {
    GoTo(GoToOutput),
    GoToAll(GoToAllOutput),
    Highlights(HighlightsOutput),
    Outline(OutlineOutput),
    NavigationBar(NavigationBarOutput),
    CallHierarchy(CallHierarchyOutput),
    TypeHierarchy(TypeHierarchyOutput),
    References(ReferencesViewOutput),
}

impl NavigationOutput {
    pub fn to_json(&self) -> Value {
        let mut v = match self {
            NavigationOutput::GoTo(o) => serde_json::to_value(o),
            NavigationOutput::GoToAll(o) => serde_json::to_value(o),
            NavigationOutput::Highlights(o) => serde_json::to_value(o),
            NavigationOutput::Outline(o) => serde_json::to_value(o),
            NavigationOutput::NavigationBar(o) => serde_json::to_value(o),
            NavigationOutput::CallHierarchy(o) => serde_json::to_value(o),
            NavigationOutput::TypeHierarchy(o) => serde_json::to_value(o),
            NavigationOutput::References(o) => serde_json::to_value(o),
        }
        .expect("navigation outputs serialize");
        // Caps the schemas state.
        if let NavigationOutput::GoTo(_) = self
            && let Some(t) = v["targets"].as_array_mut()
        {
            t.truncate(MAX_TARGETS);
        }
        if let NavigationOutput::References(_) = self
            && let Some(r) = v["references"].as_array_mut()
        {
            r.truncate(MAX_REFERENCES);
        }
        v
    }

    /// True while the answer still waits on a language server (an agent's call waits for it).
    pub fn is_loading(&self) -> bool {
        let state = match self {
            NavigationOutput::GoTo(o) => o.state,
            NavigationOutput::GoToAll(o) => o.state,
            NavigationOutput::Highlights(o) => o.state,
            NavigationOutput::Outline(o) => o.state,
            NavigationOutput::NavigationBar(o) => o.state,
            NavigationOutput::CallHierarchy(o) => o.state,
            NavigationOutput::TypeHierarchy(o) => o.state,
            NavigationOutput::References(_) => return false,
        };
        state == State::Loading
    }
}

/// Whatever answers the navigation commands (the shell). Called on the invoking thread.
pub trait NavigationCommands: Send + Sync {
    fn apply(&self, request: NavigationRequest) -> Result<NavigationOutput, CommandError>;
}

/// Register every navigation command, applying them to `target`.
pub fn register(registry: &CommandRegistry, target: Arc<dyn NavigationCommands>) {
    for id in ALL {
        let target = target.clone();
        registry.replace(spec(id), move |input| {
            let request = parse(id, input)?;
            target.apply(request).map(|out| out.to_json())
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Checks `value` against the subset of JSON Schema these files use (`$ref` into `$defs` included).
    fn conforms(root: &Value, schema: &Value, value: &Value, at: &str) {
        if let Some(r) = schema.get("$ref").and_then(Value::as_str) {
            let name = r.trim_start_matches("#/$defs/");
            return conforms(root, &root["$defs"][name], value, at);
        }
        match schema.get("type").and_then(Value::as_str) {
            Some("object") => {
                let obj = value
                    .as_object()
                    .unwrap_or_else(|| panic!("{at}: not an object"));
                for r in schema["required"].as_array().into_iter().flatten() {
                    assert!(obj.contains_key(r.as_str().unwrap()), "{at}: missing {r}");
                }
                let props = schema["properties"].as_object().unwrap();
                for (k, v) in obj {
                    let p = props
                        .get(k)
                        .unwrap_or_else(|| panic!("{at}: unexpected property {k}"));
                    conforms(root, p, v, &format!("{at}.{k}"));
                }
            }
            Some("array") => {
                let a = value
                    .as_array()
                    .unwrap_or_else(|| panic!("{at}: not an array"));
                if let Some(m) = schema.get("maxItems").and_then(Value::as_u64) {
                    assert!(a.len() as u64 <= m, "{at}: too many items");
                }
                for (i, v) in a.iter().enumerate() {
                    conforms(root, &schema["items"], v, &format!("{at}[{i}]"));
                }
            }
            Some("string") => assert!(value.is_string(), "{at}"),
            Some("boolean") => assert!(value.is_boolean(), "{at}"),
            Some("integer") => assert!(value.is_u64(), "{at}"),
            Some("number") => assert!(value.is_number(), "{at}"),
            _ => {}
        }
        if let Some(e) = schema.get("enum").and_then(Value::as_array) {
            assert!(e.contains(value), "{at}: {value} not in {e:?}");
        }
    }

    fn check(id: &str, out: &NavigationOutput) {
        let schema = spec(id).output_schema;
        conforms(&schema, &schema, &out.to_json(), id);
    }

    #[test]
    fn every_command_has_a_read_spec_visible_to_agents() {
        for id in ALL {
            let s = spec(id);
            assert_eq!(s.permission, PermissionClass::Read, "{id}");
            assert!(s.agent_visible, "{id}");
            assert_eq!(s.input_schema["additionalProperties"], json!(false), "{id}");
        }
    }

    #[test]
    fn inputs_parse_and_bad_ones_are_refused() {
        assert_eq!(
            parse(GO_TO_IMPLEMENTATION, json!({"path": "/a.cs", "line": 3})).unwrap(),
            NavigationRequest::GoTo {
                kind: GoToKind::Implementation,
                at: At {
                    path: Some("/a.cs".into()),
                    line: Some(3),
                    column: None
                },
                target: None
            }
        );
        assert!(matches!(
            parse(GO_TO_BASE, Value::Null).unwrap(),
            NavigationRequest::GoTo {
                kind: GoToKind::Base,
                ..
            }
        ));
        assert!(matches!(
            parse(GO_TO_DERIVED, json!({"target": 1})).unwrap(),
            NavigationRequest::GoTo {
                target: Some(1),
                ..
            }
        ));
        assert!(matches!(
            parse(GO_TO_ALL, json!({"query": "t Order", "max_results": 5})).unwrap(),
            NavigationRequest::GoToAll { max_results: 5, .. }
        ));
        assert!(matches!(
            parse(
                CALL_HIERARCHY,
                json!({"expand": 3, "direction": "calls_from"})
            )
            .unwrap(),
            NavigationRequest::CallHierarchy {
                expand: Some((3, CallDirection::CallsFrom)),
                ..
            }
        ));
        assert!(matches!(
            parse(
                NAVIGATION_BAR,
                json!({"select": {"dropdown": "members", "index": 2}})
            )
            .unwrap(),
            NavigationRequest::NavigationBar {
                select: Some((Dropdown::Members, 2)),
                ..
            }
        ));
        assert!(matches!(
            parse(
                REFERENCES,
                json!({"kinds": ["write"], "group_by": "file", "navigate": "next"})
            )
            .unwrap(),
            NavigationRequest::References {
                navigate: Some(Step::Next),
                group_by: Some(GroupBy::File),
                ..
            }
        ));
        for (id, bad) in [
            (GO_TO_IMPLEMENTATION, json!({"line": 0})),
            (GO_TO_IMPLEMENTATION, json!({"column": 2})),
            (GO_TO_IMPLEMENTATION, json!({"target": 0, "line": 2})),
            (GO_TO_TYPE_DEFINITION, json!({"path": ""})),
            (GO_TO_ALL, json!({"max_results": 0})),
            (GO_TO_ALL, json!({"max_results": 501})),
            (GO_TO_ALL, json!({"filter": "everything"})),
            (HIGHLIGHT_REFERENCES, json!({"navigate": "sideways"})),
            (DOCUMENT_OUTLINE, json!({"depth": 2})),
            (FILE_STRUCTURE, json!({"select": 1, "close": true})),
            (
                NAVIGATION_BAR,
                json!({"select": {"dropdown": "projects", "index": 0}}),
            ),
            (CALL_HIERARCHY, json!({"expand": 1, "navigate": 1})),
            (CALL_HIERARCHY, json!({"site": 1})),
            (CALL_HIERARCHY, json!({"direction": "calls_to"})),
            (TYPE_HIERARCHY, json!({"direction": "base_types"})),
            (REFERENCES, json!({"kinds": ["comment"]})),
            (REFERENCES, json!({"group_by": "symbol"})),
        ] {
            assert!(parse(id, bad.clone()).is_err(), "{id} {bad}");
        }
    }

    #[test]
    fn query_prefixes_select_visual_studio_filters() {
        assert_eq!(Filter::from_query("f prog"), (Some(Filter::Files), "prog"));
        assert_eq!(
            Filter::from_query("t Order"),
            (Some(Filter::Types), "Order")
        );
        assert_eq!(
            Filter::from_query("m Main"),
            (Some(Filter::Members), "Main")
        );
        assert_eq!(Filter::from_query("# x"), (Some(Filter::Symbols), "x"));
        assert_eq!(Filter::from_query(":42"), (Some(Filter::Line), "42"));
        assert_eq!(Filter::from_query("fOrder"), (None, "fOrder"));
        assert!(Filter::Types.keeps_symbol(5) && !Filter::Types.keeps_symbol(6));
        assert!(Filter::Members.keeps_symbol(6) && !Filter::Members.keeps_symbol(5));
        assert!(Filter::All.wants_files() && Filter::All.wants_symbols());
        assert!(!Filter::Types.wants_files() && !Filter::Files.wants_symbols());
    }

    #[test]
    fn outputs_conform_to_their_schemas() {
        let target = GoToTarget {
            path: "/a.cs".into(),
            line: 1,
            column: 2,
            metadata: false,
            generated: false,
            title: "a.cs".into(),
        };
        for id in [GO_TO_IMPLEMENTATION, GO_TO_BASE] {
            check(
                id,
                &NavigationOutput::GoTo(GoToOutput {
                    kind: GoToKind::Implementation,
                    path: "/a.cs".into(),
                    line: 1,
                    column: 1,
                    state: State::Navigated,
                    targets: vec![target.clone(); 120],
                    navigated: Some(target.clone()),
                    message: None,
                }),
            );
        }
        check(
            GO_TO_ALL,
            &NavigationOutput::GoToAll(GoToAllOutput {
                query: "Ord".into(),
                filter: Filter::All,
                state: State::Loading,
                results: vec![GoToAllResult {
                    name: "Order.cs".into(),
                    kind: "file".into(),
                    container: Some("src/App/Models".into()),
                    path: "/w/src/App/Models/Order.cs".into(),
                    line: None,
                    column: None,
                    score: 0.8,
                    server: None,
                }],
                total: 1,
                servers_pending: 1,
                message: None,
            }),
        );
        check(
            HIGHLIGHT_REFERENCES,
            &NavigationOutput::Highlights(HighlightsOutput {
                path: "/a.cs".into(),
                state: State::Done,
                highlights: vec![HighlightOut {
                    line: 1,
                    column: 1,
                    end_line: 1,
                    end_column: 4,
                    kind: HighlightKind::Write,
                }],
                current: Some(0),
                message: None,
            }),
        );
        let symbol = SymbolOut {
            name: "Order".into(),
            kind: "class".into(),
            detail: None,
            line: 1,
            column: 7,
            end_line: 3,
            children: vec![SymbolOut {
                name: "Total".into(),
                kind: "property".into(),
                detail: Some("decimal".into()),
                line: 2,
                column: 5,
                end_line: 2,
                children: vec![],
            }],
        };
        let outline = OutlineOutput {
            path: "/a.cs".into(),
            state: State::Done,
            symbols: vec![symbol],
            current: vec!["Order".into()],
            filter: Some("Tot".into()),
            message: None,
            open: None,
        };
        check(
            DOCUMENT_OUTLINE,
            &NavigationOutput::Outline(outline.clone()),
        );
        check(
            FILE_STRUCTURE,
            &NavigationOutput::Outline(OutlineOutput {
                open: Some(true),
                ..outline
            }),
        );
        let entry = BarEntry {
            name: "Order".into(),
            kind: "class".into(),
            line: 1,
            column: 7,
        };
        check(
            NAVIGATION_BAR,
            &NavigationOutput::NavigationBar(NavigationBarOutput {
                path: "/a.cs".into(),
                visible: true,
                project: "App".into(),
                types: vec![entry.clone()],
                members: vec![entry],
                type_index: Some(0),
                member_index: None,
                state: State::Done,
            }),
        );
        let leaf = CallNodeOut {
            id: 2,
            name: "Main".into(),
            kind: "method".into(),
            detail: Some("Program".into()),
            path: "/p.cs".into(),
            line: 3,
            column: 17,
            sites: vec![SiteOut {
                path: "/p.cs".into(),
                line: 3,
                column: 30,
                text: Some("Run();".into()),
            }],
            calls_to: None,
            calls_from: None,
        };
        check(
            CALL_HIERARCHY,
            &NavigationOutput::CallHierarchy(CallHierarchyOutput {
                state: State::Done,
                roots: vec![CallNodeOut {
                    id: 1,
                    calls_to: Some(vec![leaf]),
                    sites: vec![],
                    ..CallNodeOut {
                        id: 0,
                        name: "Run".into(),
                        kind: "method".into(),
                        detail: None,
                        path: "/p.cs".into(),
                        line: 5,
                        column: 10,
                        sites: vec![],
                        calls_to: None,
                        calls_from: None,
                    }
                }],
                navigated: None,
                message: None,
            }),
        );
        check(
            TYPE_HIERARCHY,
            &NavigationOutput::TypeHierarchy(TypeHierarchyOutput {
                state: State::Done,
                roots: vec![TypeNodeOut {
                    id: 0,
                    name: "Order".into(),
                    kind: "class".into(),
                    detail: None,
                    path: "/o.cs".into(),
                    line: 1,
                    column: 7,
                    base_types: Some(vec![]),
                    derived_types: None,
                }],
                navigated: Some(false),
                message: None,
            }),
        );
        check(
            REFERENCES,
            &NavigationOutput::References(ReferencesViewOutput {
                symbol: "Order".into(),
                kinds: ReferenceKind::ALL.to_vec(),
                group_by: GroupBy::ProjectFile,
                search: String::new(),
                keep: false,
                shown: 1,
                total: 1,
                references: vec![ReferenceOut {
                    project: Some("App".into()),
                    path: "/o.cs".into(),
                    line: 1,
                    column: 7,
                    text: "class Order { }".into(),
                    kind: ReferenceKind::Definition,
                    kept: false,
                }],
                selected: Some(0),
            }),
        );
    }
}
