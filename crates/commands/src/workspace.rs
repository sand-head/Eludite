//! Solution, file and editor commands (PLAN.md 4.1, 4.2, 5.1; brief 0012): `eludite.solution.open` and `close`,
//! `eludite.file.open` and `close`, and the editor actions that must be commands, `eludite.editor.save`, `undo`,
//! `redo` and `find`. Brief 0013 adds IntelliSense: `eludite.editor.complete`, `accept_completion`, `hover` and
//! `signature_help`, which the keys, typing and the mouse run too, so an agent can drive and observe them. Brief 0014
//! adds navigation: `eludite.editor.go_to_definition` (F12, Ctrl+click), `eludite.editor.find_references`
//! (Shift+F12), `eludite.navigation.back` and `forward` (Ctrl+-, Ctrl+Shift+-), and the Error List's toolbar,
//! `eludite.error_list.filter`.
//!
//! The schemas are the files in `protocol/schemas/` (checked in first, CLAUDE.md invariant 4), embedded at compile
//! time. This module parses and validates input into a typed [`WorkspaceRequest`] and serializes the typed
//! [`WorkspaceOutput`]; the shell implements [`WorkspaceTarget`]. File > Open Project/Solution, Solution Explorer,
//! the document tabs, the Error List, Ctrl+S and agents all reach the workspace through these commands.

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{CommandError, CommandId, CommandRegistry, CommandSpec, PermissionClass};

pub const SOLUTION_OPEN: &str = "eludite.solution.open";
pub const SOLUTION_CLOSE: &str = "eludite.solution.close";
pub const FILE_OPEN: &str = "eludite.file.open";
pub const FILE_CLOSE: &str = "eludite.file.close";
pub const EDITOR_SAVE: &str = "eludite.editor.save";
pub const EDITOR_UNDO: &str = "eludite.editor.undo";
pub const EDITOR_REDO: &str = "eludite.editor.redo";
pub const EDITOR_FIND: &str = "eludite.editor.find";
pub const EDITOR_COMPLETE: &str = "eludite.editor.complete";
pub const EDITOR_ACCEPT_COMPLETION: &str = "eludite.editor.accept_completion";
pub const EDITOR_HOVER: &str = "eludite.editor.hover";
pub const EDITOR_SIGNATURE_HELP: &str = "eludite.editor.signature_help";
pub const EDITOR_GO_TO_DEFINITION: &str = "eludite.editor.go_to_definition";
pub const EDITOR_FIND_REFERENCES: &str = "eludite.editor.find_references";
pub const NAVIGATION_BACK: &str = "eludite.navigation.back";
pub const NAVIGATION_FORWARD: &str = "eludite.navigation.forward";
pub const ERROR_LIST_FILTER: &str = "eludite.error_list.filter";

/// Every command this module registers.
pub const ALL: [&str; 17] = [
    SOLUTION_OPEN,
    SOLUTION_CLOSE,
    FILE_OPEN,
    FILE_CLOSE,
    EDITOR_SAVE,
    EDITOR_UNDO,
    EDITOR_REDO,
    EDITOR_FIND,
    EDITOR_COMPLETE,
    EDITOR_ACCEPT_COMPLETION,
    EDITOR_HOVER,
    EDITOR_SIGNATURE_HELP,
    EDITOR_GO_TO_DEFINITION,
    EDITOR_FIND_REFERENCES,
    NAVIGATION_BACK,
    NAVIGATION_FORWARD,
    ERROR_LIST_FILTER,
];

const HISTORY_OUTPUT: &str = include_str!("../../../protocol/schemas/editor-history.output.json");
const NAVIGATION_OUTPUT: &str = include_str!("../../../protocol/schemas/navigation.output.json");

/// (title, input schema, output schema, permission)
fn schemas(id: &str) -> (&'static str, &'static str, &'static str, PermissionClass) {
    use PermissionClass::*;
    match id {
        // Loading runs MSBuild evaluations and design-time builds (PLAN.md 5.3, execute).
        SOLUTION_OPEN => (
            "File: Open Project/Solution",
            include_str!("../../../protocol/schemas/solution-open.input.json"),
            include_str!("../../../protocol/schemas/solution-open.output.json"),
            Execute,
        ),
        SOLUTION_CLOSE => (
            "File: Close Solution",
            include_str!("../../../protocol/schemas/solution-close.input.json"),
            include_str!("../../../protocol/schemas/solution-close.output.json"),
            Read,
        ),
        FILE_OPEN => (
            "File: Open",
            include_str!("../../../protocol/schemas/file-open.input.json"),
            include_str!("../../../protocol/schemas/file-open.output.json"),
            Read,
        ),
        // Closing can save or drop unsaved changes.
        FILE_CLOSE => (
            "File: Close",
            include_str!("../../../protocol/schemas/file-close.input.json"),
            include_str!("../../../protocol/schemas/file-close.output.json"),
            EditBuffer,
        ),
        EDITOR_SAVE => (
            "File: Save",
            include_str!("../../../protocol/schemas/editor-save.input.json"),
            include_str!("../../../protocol/schemas/editor-save.output.json"),
            EditBuffer,
        ),
        EDITOR_UNDO => (
            "Edit: Undo",
            include_str!("../../../protocol/schemas/editor-undo.input.json"),
            HISTORY_OUTPUT,
            EditBuffer,
        ),
        EDITOR_REDO => (
            "Edit: Redo",
            include_str!("../../../protocol/schemas/editor-redo.input.json"),
            HISTORY_OUTPUT,
            EditBuffer,
        ),
        EDITOR_FIND => (
            "Edit: Find",
            include_str!("../../../protocol/schemas/editor-find.input.json"),
            include_str!("../../../protocol/schemas/editor-find.output.json"),
            Read,
        ),
        EDITOR_COMPLETE => (
            "Edit: IntelliSense: Complete Word",
            include_str!("../../../protocol/schemas/editor-complete.input.json"),
            include_str!("../../../protocol/schemas/editor-complete.output.json"),
            Read,
        ),
        // Committing an item edits the document.
        EDITOR_ACCEPT_COMPLETION => (
            "Edit: IntelliSense: Commit Completion",
            include_str!("../../../protocol/schemas/editor-accept-completion.input.json"),
            include_str!("../../../protocol/schemas/editor-accept-completion.output.json"),
            EditBuffer,
        ),
        EDITOR_HOVER => (
            "Edit: IntelliSense: Quick Info",
            include_str!("../../../protocol/schemas/editor-hover.input.json"),
            include_str!("../../../protocol/schemas/editor-hover.output.json"),
            Read,
        ),
        EDITOR_SIGNATURE_HELP => (
            "Edit: IntelliSense: Parameter Info",
            include_str!("../../../protocol/schemas/editor-signature-help.input.json"),
            include_str!("../../../protocol/schemas/editor-signature-help.output.json"),
            Read,
        ),
        EDITOR_GO_TO_DEFINITION => (
            "Edit: Go To Definition",
            include_str!("../../../protocol/schemas/editor-go-to-definition.input.json"),
            include_str!("../../../protocol/schemas/editor-go-to-definition.output.json"),
            Read,
        ),
        EDITOR_FIND_REFERENCES => (
            "Edit: Find All References",
            include_str!("../../../protocol/schemas/editor-find-references.input.json"),
            include_str!("../../../protocol/schemas/editor-find-references.output.json"),
            Read,
        ),
        NAVIGATION_BACK => (
            "View: Navigate Backward",
            include_str!("../../../protocol/schemas/navigation-back.input.json"),
            NAVIGATION_OUTPUT,
            Read,
        ),
        NAVIGATION_FORWARD => (
            "View: Navigate Forward",
            include_str!("../../../protocol/schemas/navigation-forward.input.json"),
            NAVIGATION_OUTPUT,
            Read,
        ),
        // A view filter: what the window shows, never what diagnostics.list returns.
        ERROR_LIST_FILTER => (
            "Error List: Filter",
            include_str!("../../../protocol/schemas/error-list-filter.input.json"),
            include_str!("../../../protocol/schemas/error-list-filter.output.json"),
            Read,
        ),
        other => unreachable!("not a workspace command: {other}"),
    }
}

/// What `eludite.file.close` does with unsaved changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CloseSave {
    Save,
    Discard,
}

/// A parsed, validated workspace command. `path: None` on an editor command means the active document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkspaceRequest {
    SolutionOpen {
        path: String,
    },
    SolutionClose,
    FileOpen {
        path: String,
        /// 1-based.
        line: Option<u32>,
        /// 1-based.
        column: Option<u32>,
    },
    FileClose {
        path: String,
        save: Option<CloseSave>,
    },
    Save {
        path: Option<String>,
    },
    Undo {
        path: Option<String>,
    },
    Redo {
        path: Option<String>,
    },
    Find {
        path: Option<String>,
        query: Option<String>,
        case_sensitive: bool,
    },
    /// Completion at the caret (after moving it to `line`, `column`); `trigger` is the character just typed.
    Complete {
        path: Option<String>,
        line: Option<u32>,
        column: Option<u32>,
        trigger: Option<char>,
    },
    AcceptCompletion {
        path: Option<String>,
        label: Option<String>,
    },
    /// Quick Info at `line`, `column` (the caret when `line` is `None`).
    Hover {
        path: Option<String>,
        line: Option<u32>,
        column: Option<u32>,
    },
    SignatureHelp {
        path: Option<String>,
        line: Option<u32>,
        column: Option<u32>,
        trigger: Option<char>,
    },
    /// Go To Definition at `line`, `column` (the caret when `line` is `None`), or choose `target` of the open
    /// picker.
    GoToDefinition {
        path: Option<String>,
        line: Option<u32>,
        column: Option<u32>,
        target: Option<usize>,
    },
    FindReferences {
        path: Option<String>,
        line: Option<u32>,
        column: Option<u32>,
    },
    NavigateBack,
    NavigateForward,
    /// The Error List's filters; `None` keeps the current value. `project: Some(None)` shows every project.
    ErrorListFilter(ErrorListFilterInput),
}

/// `error-list-filter.input.json`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ErrorListFilterInput {
    pub errors: Option<bool>,
    pub warnings: Option<bool>,
    pub messages: Option<bool>,
    pub project: Option<Option<String>>,
    pub text: Option<String>,
}

impl WorkspaceRequest {
    /// The command id this request came from.
    pub fn command(&self) -> &'static str {
        match self {
            WorkspaceRequest::SolutionOpen { .. } => SOLUTION_OPEN,
            WorkspaceRequest::SolutionClose => SOLUTION_CLOSE,
            WorkspaceRequest::FileOpen { .. } => FILE_OPEN,
            WorkspaceRequest::FileClose { .. } => FILE_CLOSE,
            WorkspaceRequest::Save { .. } => EDITOR_SAVE,
            WorkspaceRequest::Undo { .. } => EDITOR_UNDO,
            WorkspaceRequest::Redo { .. } => EDITOR_REDO,
            WorkspaceRequest::Find { .. } => EDITOR_FIND,
            WorkspaceRequest::Complete { .. } => EDITOR_COMPLETE,
            WorkspaceRequest::AcceptCompletion { .. } => EDITOR_ACCEPT_COMPLETION,
            WorkspaceRequest::Hover { .. } => EDITOR_HOVER,
            WorkspaceRequest::SignatureHelp { .. } => EDITOR_SIGNATURE_HELP,
            WorkspaceRequest::GoToDefinition { .. } => EDITOR_GO_TO_DEFINITION,
            WorkspaceRequest::FindReferences { .. } => EDITOR_FIND_REFERENCES,
            WorkspaceRequest::NavigateBack => NAVIGATION_BACK,
            WorkspaceRequest::NavigateForward => NAVIGATION_FORWARD,
            WorkspaceRequest::ErrorListFilter(_) => ERROR_LIST_FILTER,
        }
    }

    /// The document an editor command names (`None`: the active one), for commands that have a path.
    pub fn path(&self) -> Option<&str> {
        match self {
            WorkspaceRequest::Save { path }
            | WorkspaceRequest::Undo { path }
            | WorkspaceRequest::Redo { path }
            | WorkspaceRequest::Find { path, .. }
            | WorkspaceRequest::Complete { path, .. }
            | WorkspaceRequest::AcceptCompletion { path, .. }
            | WorkspaceRequest::Hover { path, .. }
            | WorkspaceRequest::SignatureHelp { path, .. }
            | WorkspaceRequest::GoToDefinition { path, .. }
            | WorkspaceRequest::FindReferences { path, .. } => path.as_deref(),
            WorkspaceRequest::FileOpen { path, .. } | WorkspaceRequest::FileClose { path, .. } => {
                Some(path)
            }
            WorkspaceRequest::SolutionOpen { .. }
            | WorkspaceRequest::SolutionClose
            | WorkspaceRequest::NavigateBack
            | WorkspaceRequest::NavigateForward
            | WorkspaceRequest::ErrorListFilter(_) => None,
        }
    }
}

/// `solution-open.output.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SolutionOpenOutput {
    pub path: String,
    /// Always `"loading"`.
    pub state: String,
}

/// `solution-close.output.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SolutionCloseOutput {
    pub closed: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
}

/// `file-open.output.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FileOpenOutput {
    pub path: String,
    pub already_open: bool,
}

/// `file-close.output.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FileCloseOutput {
    pub path: String,
    pub closed: bool,
    pub saved: bool,
}

/// `editor-save.output.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SaveOutput {
    pub path: String,
    pub bytes: u64,
}

/// `editor-history.output.json` (undo and redo).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HistoryOutput {
    pub path: String,
    pub applied: bool,
    pub dirty: bool,
}

/// `editor-find.output.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FindOutput {
    pub path: String,
    pub found: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub column: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub find_bar_open: Option<bool>,
}

/// The state of an IntelliSense popup in the `eludite.editor.*` outputs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PopupState {
    Loading,
    Open,
    Closed,
}

/// One row of `editor-complete.output.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CompletionRow {
    pub label: String,
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

/// `editor-complete.output.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CompleteOutput {
    pub path: String,
    pub line: u32,
    pub column: u32,
    pub state: PopupState,
    /// `languageServer` or `syntax`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    pub filter: String,
    pub total: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selected: Option<String>,
    /// At most [`MAX_COMPLETION_ROWS`].
    pub items: Vec<CompletionRow>,
}

/// Rows `eludite.editor.complete` reports at most.
pub const MAX_COMPLETION_ROWS: usize = 100;

/// `editor-accept-completion.output.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AcceptCompletionOutput {
    pub path: String,
    pub accepted: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    pub line: u32,
    pub column: u32,
}

/// `editor-hover.output.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HoverOutput {
    pub path: String,
    pub line: u32,
    pub column: u32,
    pub state: PopupState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
}

/// One overload in `editor-signature-help.output.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SignatureRow {
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub documentation: Option<String>,
    pub parameters: Vec<String>,
}

/// `editor-signature-help.output.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SignatureHelpOutput {
    pub path: String,
    pub line: u32,
    pub column: u32,
    pub state: PopupState,
    pub signatures: Vec<SignatureRow>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_signature: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_parameter: Option<u32>,
}

/// What Go To Definition did (`editor-go-to-definition.output.json`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DefinitionState {
    Loading,
    Navigated,
    Choose,
    None,
    Failed,
}

/// One definition in `editor-go-to-definition.output.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DefinitionTarget {
    pub path: String,
    pub line: u32,
    pub column: u32,
    pub metadata: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
}

/// `editor-go-to-definition.output.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DefinitionOutput {
    pub path: String,
    pub line: u32,
    pub column: u32,
    pub state: DefinitionState,
    /// At most [`MAX_DEFINITION_TARGETS`].
    pub targets: Vec<DefinitionTarget>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub navigated: Option<DefinitionTarget>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

/// Targets `eludite.editor.go_to_definition` reports at most.
pub const MAX_DEFINITION_TARGETS: usize = 100;

/// The Find All References window's state (`editor-find-references.output.json`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ReferencesState {
    Loading,
    Done,
    Failed,
}

/// One reference in `editor-find-references.output.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReferenceRow {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    pub path: String,
    pub line: u32,
    pub column: u32,
    pub text: String,
}

/// `editor-find-references.output.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReferencesOutput {
    pub path: String,
    pub line: u32,
    pub column: u32,
    pub state: ReferencesState,
    pub symbol: String,
    pub total: u64,
    /// At most [`MAX_REFERENCE_ROWS`].
    pub references: Vec<ReferenceRow>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

/// References `eludite.editor.find_references` reports at most (the window shows them all).
pub const MAX_REFERENCE_ROWS: usize = 1000;

/// `navigation.output.json` (back and forward).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NavigationOutput {
    pub navigated: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub column: Option<u32>,
    pub back: u32,
    pub forward: u32,
}

/// The Error List's toggle-button counts in `error-list-filter.output.json`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FilterCounts {
    pub errors: u64,
    pub warnings: u64,
    pub messages: u64,
}

/// `error-list-filter.output.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ErrorListFilterOutput {
    pub errors: bool,
    pub warnings: bool,
    pub messages: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    pub text: String,
    pub counts: FilterCounts,
    pub shown: u64,
    pub total: u64,
}

/// The typed result of a workspace command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkspaceOutput {
    SolutionOpen(SolutionOpenOutput),
    SolutionClose(SolutionCloseOutput),
    FileOpen(FileOpenOutput),
    FileClose(FileCloseOutput),
    Save(SaveOutput),
    History(HistoryOutput),
    Find(FindOutput),
    Complete(CompleteOutput),
    AcceptCompletion(AcceptCompletionOutput),
    Hover(HoverOutput),
    SignatureHelp(SignatureHelpOutput),
    GoToDefinition(DefinitionOutput),
    FindReferences(ReferencesOutput),
    Navigation(NavigationOutput),
    ErrorListFilter(ErrorListFilterOutput),
}

impl WorkspaceOutput {
    pub fn to_json(&self) -> Value {
        match self {
            WorkspaceOutput::SolutionOpen(o) => serde_json::to_value(o),
            WorkspaceOutput::SolutionClose(o) => serde_json::to_value(o),
            WorkspaceOutput::FileOpen(o) => serde_json::to_value(o),
            WorkspaceOutput::FileClose(o) => serde_json::to_value(o),
            WorkspaceOutput::Save(o) => serde_json::to_value(o),
            WorkspaceOutput::History(o) => serde_json::to_value(o),
            WorkspaceOutput::Find(o) => serde_json::to_value(o),
            WorkspaceOutput::Complete(o) => serde_json::to_value(o),
            WorkspaceOutput::AcceptCompletion(o) => serde_json::to_value(o),
            WorkspaceOutput::Hover(o) => serde_json::to_value(o),
            WorkspaceOutput::SignatureHelp(o) => serde_json::to_value(o),
            WorkspaceOutput::GoToDefinition(o) => serde_json::to_value(o),
            WorkspaceOutput::FindReferences(o) => serde_json::to_value(o),
            WorkspaceOutput::Navigation(o) => serde_json::to_value(o),
            WorkspaceOutput::ErrorListFilter(o) => serde_json::to_value(o),
        }
        .expect("workspace outputs serialize")
    }

    /// True for an IntelliSense output still waiting for its answer.
    pub fn is_loading(&self) -> bool {
        matches!(
            self,
            WorkspaceOutput::Complete(CompleteOutput {
                state: PopupState::Loading,
                ..
            }) | WorkspaceOutput::Hover(HoverOutput {
                state: PopupState::Loading,
                ..
            }) | WorkspaceOutput::SignatureHelp(SignatureHelpOutput {
                state: PopupState::Loading,
                ..
            }) | WorkspaceOutput::GoToDefinition(DefinitionOutput {
                state: DefinitionState::Loading,
                ..
            }) | WorkspaceOutput::FindReferences(ReferencesOutput {
                state: ReferencesState::Loading,
                ..
            })
        )
    }
}

/// Whatever owns the solution and the editors (the shell). Called on the invoking thread: the UI thread for menus,
/// keys and clicks, a server thread for agents.
pub trait WorkspaceTarget: Send + Sync {
    fn apply(&self, request: WorkspaceRequest) -> Result<WorkspaceOutput, CommandError>;
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PathIn {
    path: String,
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct Empty {}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FileOpenIn {
    path: String,
    line: Option<u32>,
    column: Option<u32>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FileCloseIn {
    path: String,
    save: Option<CloseSave>,
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct DocIn {
    path: Option<String>,
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct PositionIn {
    path: Option<String>,
    line: Option<u32>,
    column: Option<u32>,
    trigger: Option<String>,
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct HoverIn {
    path: Option<String>,
    line: Option<u32>,
    column: Option<u32>,
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct AcceptIn {
    path: Option<String>,
    label: Option<String>,
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct DefinitionIn {
    path: Option<String>,
    line: Option<u32>,
    column: Option<u32>,
    target: Option<usize>,
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct FilterIn {
    errors: Option<bool>,
    warnings: Option<bool>,
    messages: Option<bool>,
    /// Absent: keep; null: every project (handled in `parse`).
    #[serde(default, deserialize_with = "present")]
    project: Option<Option<String>>,
    text: Option<String>,
}

/// A member that may be absent (`None`), null (`Some(None)`) or a value.
fn present<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<Option<String>>, D::Error> {
    Option::<String>::deserialize(d).map(Some)
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct FindIn {
    path: Option<String>,
    query: Option<String>,
    case_sensitive: Option<bool>,
}

fn input<T: serde::de::DeserializeOwned + Default>(input: Value) -> Result<T, CommandError> {
    if input.is_null() {
        return Ok(T::default());
    }
    serde_json::from_value(input).map_err(|e| CommandError::InvalidInput(e.to_string()))
}

fn required<T: serde::de::DeserializeOwned>(input: Value) -> Result<T, CommandError> {
    serde_json::from_value(input).map_err(|e| CommandError::InvalidInput(e.to_string()))
}

fn non_empty(field: &str, s: &str) -> Result<(), CommandError> {
    if s.is_empty() {
        Err(CommandError::InvalidInput(format!(
            "`{field}` must not be empty"
        )))
    } else {
        Ok(())
    }
}

fn position(line: Option<u32>, column: Option<u32>) -> Result<(), CommandError> {
    if line == Some(0) || column == Some(0) {
        return Err(CommandError::InvalidInput(
            "`line` and `column` are 1-based".into(),
        ));
    }
    if line.is_none() && column.is_some() {
        return Err(CommandError::InvalidInput("`column` needs `line`".into()));
    }
    Ok(())
}

fn one_char(trigger: Option<String>) -> Result<Option<char>, CommandError> {
    match trigger {
        None => Ok(None),
        Some(t) => {
            let mut chars = t.chars();
            match (chars.next(), chars.next()) {
                (Some(c), None) => Ok(Some(c)),
                _ => Err(CommandError::InvalidInput(
                    "`trigger` is one character".into(),
                )),
            }
        }
    }
}

fn optional_path(path: Option<String>) -> Result<Option<String>, CommandError> {
    if let Some(p) = &path {
        non_empty("path", p)?;
    }
    Ok(path)
}

/// Parse and validate the input of workspace command `id`.
pub fn parse(id: &str, value: Value) -> Result<WorkspaceRequest, CommandError> {
    Ok(match id {
        SOLUTION_OPEN => {
            let i: PathIn = required(value)?;
            non_empty("path", &i.path)?;
            WorkspaceRequest::SolutionOpen { path: i.path }
        }
        SOLUTION_CLOSE => {
            let _: Empty = input(value)?;
            WorkspaceRequest::SolutionClose
        }
        FILE_OPEN => {
            let i: FileOpenIn = required(value)?;
            non_empty("path", &i.path)?;
            if i.line == Some(0) || i.column == Some(0) {
                return Err(CommandError::InvalidInput(
                    "`line` and `column` are 1-based".into(),
                ));
            }
            WorkspaceRequest::FileOpen {
                path: i.path,
                line: i.line,
                column: i.column,
            }
        }
        FILE_CLOSE => {
            let i: FileCloseIn = required(value)?;
            non_empty("path", &i.path)?;
            WorkspaceRequest::FileClose {
                path: i.path,
                save: i.save,
            }
        }
        EDITOR_SAVE | EDITOR_UNDO | EDITOR_REDO => {
            let i: DocIn = input(value)?;
            let path = optional_path(i.path)?;
            match id {
                EDITOR_SAVE => WorkspaceRequest::Save { path },
                EDITOR_UNDO => WorkspaceRequest::Undo { path },
                _ => WorkspaceRequest::Redo { path },
            }
        }
        EDITOR_FIND => {
            let i: FindIn = input(value)?;
            if let Some(q) = &i.query {
                non_empty("query", q)?;
            }
            WorkspaceRequest::Find {
                path: optional_path(i.path)?,
                query: i.query,
                case_sensitive: i.case_sensitive.unwrap_or(false),
            }
        }
        EDITOR_COMPLETE | EDITOR_SIGNATURE_HELP => {
            let i: PositionIn = input(value)?;
            position(i.line, i.column)?;
            let (path, line, column, trigger) = (
                optional_path(i.path)?,
                i.line,
                i.column,
                one_char(i.trigger)?,
            );
            if id == EDITOR_COMPLETE {
                WorkspaceRequest::Complete {
                    path,
                    line,
                    column,
                    trigger,
                }
            } else {
                WorkspaceRequest::SignatureHelp {
                    path,
                    line,
                    column,
                    trigger,
                }
            }
        }
        EDITOR_HOVER => {
            let i: HoverIn = input(value)?;
            position(i.line, i.column)?;
            WorkspaceRequest::Hover {
                path: optional_path(i.path)?,
                line: i.line,
                column: i.column,
            }
        }
        EDITOR_ACCEPT_COMPLETION => {
            let i: AcceptIn = input(value)?;
            if let Some(l) = &i.label {
                non_empty("label", l)?;
            }
            WorkspaceRequest::AcceptCompletion {
                path: optional_path(i.path)?,
                label: i.label,
            }
        }
        EDITOR_GO_TO_DEFINITION => {
            let i: DefinitionIn = input(value)?;
            position(i.line, i.column)?;
            if i.target.is_some() && i.line.is_some() {
                return Err(CommandError::InvalidInput(
                    "`target` chooses from the open picker; it takes no position".into(),
                ));
            }
            WorkspaceRequest::GoToDefinition {
                path: optional_path(i.path)?,
                line: i.line,
                column: i.column,
                target: i.target,
            }
        }
        EDITOR_FIND_REFERENCES => {
            let i: HoverIn = input(value)?;
            position(i.line, i.column)?;
            WorkspaceRequest::FindReferences {
                path: optional_path(i.path)?,
                line: i.line,
                column: i.column,
            }
        }
        NAVIGATION_BACK | NAVIGATION_FORWARD => {
            let _: Empty = input(value)?;
            if id == NAVIGATION_BACK {
                WorkspaceRequest::NavigateBack
            } else {
                WorkspaceRequest::NavigateForward
            }
        }
        ERROR_LIST_FILTER => {
            let i: FilterIn = input(value)?;
            WorkspaceRequest::ErrorListFilter(ErrorListFilterInput {
                errors: i.errors,
                warnings: i.warnings,
                messages: i.messages,
                project: i.project.map(|p| p.filter(|p| !p.is_empty())),
                text: i.text,
            })
        }
        other => return Err(CommandError::UnknownCommand(other.to_owned())),
    })
}

fn parse_schema(text: &str) -> Value {
    serde_json::from_str(text).expect("protocol schemas are valid JSON")
}

/// The public description of workspace command `id` (one of [`ALL`]).
pub fn spec(id: &str) -> CommandSpec {
    let (title, input, output, permission) = schemas(id);
    CommandSpec {
        id: CommandId::new(id).expect("valid id"),
        title: title.into(),
        input_schema: parse_schema(input),
        output_schema: parse_schema(output),
        permission,
    }
}

/// Register every workspace command, applying them to `target`. Replaces an earlier registration of the same id
/// (the built-in `eludite.file.open` placeholder).
pub fn register(registry: &mut CommandRegistry, target: Arc<dyn WorkspaceTarget>) {
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
    use std::sync::Mutex;

    use super::*;
    use serde_json::json;

    /// Checks `value` against the subset of JSON Schema these files use.
    fn conforms(schema: &Value, value: &Value) {
        let obj = value.as_object().expect("outputs are objects");
        for r in schema["required"].as_array().unwrap() {
            assert!(obj.contains_key(r.as_str().unwrap()), "missing {r}");
        }
        let props = schema["properties"].as_object().unwrap();
        for (k, v) in obj {
            let p = props
                .get(k)
                .unwrap_or_else(|| panic!("unexpected property {k}"));
            match p.get("type").and_then(Value::as_str) {
                Some("string") => assert!(v.is_string(), "{k}"),
                Some("boolean") => assert!(v.is_boolean(), "{k}"),
                Some("integer") => assert!(v.is_u64(), "{k}"),
                _ => {}
            }
            if let Some(c) = p.get("const") {
                assert_eq!(c, v, "{k}");
            }
        }
    }

    #[derive(Default)]
    struct Recorder(Mutex<Vec<WorkspaceRequest>>);

    impl WorkspaceTarget for Recorder {
        fn apply(&self, request: WorkspaceRequest) -> Result<WorkspaceOutput, CommandError> {
            self.0.lock().unwrap().push(request.clone());
            Ok(match request {
                WorkspaceRequest::SolutionOpen { path } => {
                    WorkspaceOutput::SolutionOpen(SolutionOpenOutput {
                        path,
                        state: "loading".into(),
                    })
                }
                _ => WorkspaceOutput::SolutionClose(SolutionCloseOutput {
                    closed: false,
                    path: None,
                }),
            })
        }
    }

    #[test]
    fn specs_use_the_protocol_schemas() {
        for id in ALL {
            let s = spec(id);
            assert_eq!(s.input_schema["type"], "object", "{id}");
            assert_eq!(s.input_schema["additionalProperties"], false, "{id}");
            assert_eq!(s.output_schema["type"], "object", "{id}");
            assert!(
                s.input_schema["$id"]
                    .as_str()
                    .unwrap()
                    .ends_with(".input.json")
            );
        }
        assert_eq!(spec(SOLUTION_OPEN).permission, PermissionClass::Execute);
        assert_eq!(spec(EDITOR_SAVE).permission, PermissionClass::EditBuffer);
        assert_eq!(spec(EDITOR_FIND).permission, PermissionClass::Read);
        assert_eq!(spec(FILE_OPEN).input_schema["required"], json!(["path"]));
    }

    #[test]
    fn parses_every_command() {
        let p = |id, v| parse(id, v).unwrap();
        assert_eq!(
            p(SOLUTION_OPEN, json!({"path": "/s/App.slnx"})),
            WorkspaceRequest::SolutionOpen {
                path: "/s/App.slnx".into()
            }
        );
        assert_eq!(
            p(SOLUTION_CLOSE, json!({})),
            WorkspaceRequest::SolutionClose
        );
        assert_eq!(
            p(SOLUTION_CLOSE, Value::Null),
            WorkspaceRequest::SolutionClose
        );
        assert_eq!(
            p(FILE_OPEN, json!({"path": "/a.cs", "line": 3, "column": 7})),
            WorkspaceRequest::FileOpen {
                path: "/a.cs".into(),
                line: Some(3),
                column: Some(7)
            }
        );
        assert_eq!(
            p(FILE_CLOSE, json!({"path": "/a.cs", "save": "discard"})),
            WorkspaceRequest::FileClose {
                path: "/a.cs".into(),
                save: Some(CloseSave::Discard)
            }
        );
        assert_eq!(
            p(EDITOR_SAVE, json!({})),
            WorkspaceRequest::Save { path: None }
        );
        assert_eq!(
            p(EDITOR_UNDO, json!({"path": "/a.cs"})),
            WorkspaceRequest::Undo {
                path: Some("/a.cs".into())
            }
        );
        assert_eq!(
            p(EDITOR_REDO, Value::Null),
            WorkspaceRequest::Redo { path: None }
        );
        assert_eq!(
            p(
                EDITOR_FIND,
                json!({"query": "Widget", "case_sensitive": true})
            ),
            WorkspaceRequest::Find {
                path: None,
                query: Some("Widget".into()),
                case_sensitive: true
            }
        );
        assert_eq!(
            p(EDITOR_COMPLETE, json!({"trigger": ".", "line": 3})),
            WorkspaceRequest::Complete {
                path: None,
                line: Some(3),
                column: None,
                trigger: Some('.')
            }
        );
        assert_eq!(
            p(EDITOR_SIGNATURE_HELP, Value::Null),
            WorkspaceRequest::SignatureHelp {
                path: None,
                line: None,
                column: None,
                trigger: None
            }
        );
        assert_eq!(
            p(
                EDITOR_HOVER,
                json!({"path": "/a.cs", "line": 2, "column": 9})
            ),
            WorkspaceRequest::Hover {
                path: Some("/a.cs".into()),
                line: Some(2),
                column: Some(9)
            }
        );
        assert_eq!(
            p(EDITOR_ACCEPT_COMPLETION, json!({"label": "WriteLine"})),
            WorkspaceRequest::AcceptCompletion {
                path: None,
                label: Some("WriteLine".into())
            }
        );
        assert_eq!(
            p(EDITOR_HOVER, json!({})).path(),
            None,
            "the active document"
        );
        for id in ALL {
            assert!(parse(id, json!({"bogus": 1})).is_err(), "{id}");
        }
    }

    #[test]
    fn rejects_bad_input() {
        for (id, bad) in [
            (SOLUTION_OPEN, json!({})),
            (SOLUTION_OPEN, json!({"path": ""})),
            (SOLUTION_CLOSE, json!({"path": "/a"})),
            (FILE_OPEN, json!({"path": "/a", "line": 0})),
            (FILE_OPEN, json!({"path": 3})),
            (FILE_CLOSE, json!({"path": "/a", "save": "maybe"})),
            (EDITOR_SAVE, json!({"path": ""})),
            (EDITOR_FIND, json!({"query": ""})),
            (EDITOR_UNDO, json!("x")),
            (EDITOR_COMPLETE, json!({"trigger": ".."})),
            (EDITOR_COMPLETE, json!({"trigger": ""})),
            (EDITOR_COMPLETE, json!({"column": 3})),
            (EDITOR_SIGNATURE_HELP, json!({"line": 0})),
            (EDITOR_HOVER, json!({"trigger": "."})),
            (EDITOR_ACCEPT_COMPLETION, json!({"label": ""})),
        ] {
            assert!(
                matches!(parse(id, bad.clone()), Err(CommandError::InvalidInput(_))),
                "{id} {bad}"
            );
        }
    }

    #[test]
    fn outputs_match_their_schemas() {
        let cases = [
            (
                SOLUTION_OPEN,
                WorkspaceOutput::SolutionOpen(SolutionOpenOutput {
                    path: "/s/App.slnx".into(),
                    state: "loading".into(),
                }),
            ),
            (
                SOLUTION_CLOSE,
                WorkspaceOutput::SolutionClose(SolutionCloseOutput {
                    closed: true,
                    path: Some("/s/App.slnx".into()),
                }),
            ),
            (
                FILE_OPEN,
                WorkspaceOutput::FileOpen(FileOpenOutput {
                    path: "/a.cs".into(),
                    already_open: false,
                }),
            ),
            (
                FILE_CLOSE,
                WorkspaceOutput::FileClose(FileCloseOutput {
                    path: "/a.cs".into(),
                    closed: true,
                    saved: true,
                }),
            ),
            (
                EDITOR_SAVE,
                WorkspaceOutput::Save(SaveOutput {
                    path: "/a.cs".into(),
                    bytes: 12,
                }),
            ),
            (
                EDITOR_UNDO,
                WorkspaceOutput::History(HistoryOutput {
                    path: "/a.cs".into(),
                    applied: true,
                    dirty: false,
                }),
            ),
            (
                EDITOR_FIND,
                WorkspaceOutput::Find(FindOutput {
                    path: "/a.cs".into(),
                    found: true,
                    line: Some(4),
                    column: Some(9),
                    find_bar_open: None,
                }),
            ),
        ];
        let intellisense = [
            (
                EDITOR_COMPLETE,
                WorkspaceOutput::Complete(CompleteOutput {
                    path: "/a.cs".into(),
                    line: 4,
                    column: 9,
                    state: PopupState::Open,
                    source: Some("languageServer".into()),
                    filter: "Wr".into(),
                    total: 1,
                    selected: Some("WriteLine".into()),
                    items: vec![CompletionRow {
                        label: "WriteLine".into(),
                        kind: "method".into(),
                        detail: Some("void Console.WriteLine()".into()),
                    }],
                }),
            ),
            (
                EDITOR_ACCEPT_COMPLETION,
                WorkspaceOutput::AcceptCompletion(AcceptCompletionOutput {
                    path: "/a.cs".into(),
                    accepted: true,
                    label: Some("WriteLine".into()),
                    text: Some("WriteLine".into()),
                    line: 4,
                    column: 16,
                }),
            ),
            (
                EDITOR_HOVER,
                WorkspaceOutput::Hover(HoverOutput {
                    path: "/a.cs".into(),
                    line: 1,
                    column: 1,
                    state: PopupState::Loading,
                    text: None,
                }),
            ),
            (
                EDITOR_SIGNATURE_HELP,
                WorkspaceOutput::SignatureHelp(SignatureHelpOutput {
                    path: "/a.cs".into(),
                    line: 2,
                    column: 3,
                    state: PopupState::Open,
                    signatures: vec![SignatureRow {
                        label: "void M(int a)".into(),
                        documentation: None,
                        parameters: vec!["int a".into()],
                    }],
                    active_signature: Some(0),
                    active_parameter: Some(0),
                }),
            ),
        ];
        for (id, out) in cases.into_iter().chain(intellisense) {
            conforms(&spec(id).output_schema, &out.to_json());
        }
        let state = |s: PopupState| serde_json::to_value(s).unwrap();
        let schema = spec(EDITOR_COMPLETE).output_schema;
        for s in [PopupState::Loading, PopupState::Open, PopupState::Closed] {
            assert!(
                schema["properties"]["state"]["enum"]
                    .as_array()
                    .unwrap()
                    .contains(&state(s))
            );
        }
        assert_eq!(
            schema["properties"]["items"]["maxItems"],
            json!(MAX_COMPLETION_ROWS)
        );
        assert_eq!(
            spec(EDITOR_ACCEPT_COMPLETION).permission,
            PermissionClass::EditBuffer
        );
        assert_eq!(spec(EDITOR_HOVER).permission, PermissionClass::Read);
    }

    #[test]
    fn navigation_commands_parse_validate_and_match_their_schemas() {
        let p = |id, v| parse(id, v).unwrap();
        assert_eq!(
            p(EDITOR_GO_TO_DEFINITION, json!({"path": "/a.cs", "line": 3})),
            WorkspaceRequest::GoToDefinition {
                path: Some("/a.cs".into()),
                line: Some(3),
                column: None,
                target: None
            }
        );
        assert_eq!(
            p(EDITOR_GO_TO_DEFINITION, json!({"target": 1})),
            WorkspaceRequest::GoToDefinition {
                path: None,
                line: None,
                column: None,
                target: Some(1)
            }
        );
        assert_eq!(
            p(EDITOR_FIND_REFERENCES, Value::Null),
            WorkspaceRequest::FindReferences {
                path: None,
                line: None,
                column: None
            }
        );
        assert_eq!(
            p(NAVIGATION_BACK, json!({})),
            WorkspaceRequest::NavigateBack
        );
        assert_eq!(
            p(NAVIGATION_FORWARD, Value::Null),
            WorkspaceRequest::NavigateForward
        );
        // Absent keeps, null or "" is every project, a name filters.
        let filter = |v| match p(ERROR_LIST_FILTER, v) {
            WorkspaceRequest::ErrorListFilter(f) => f,
            other => panic!("{other:?}"),
        };
        assert_eq!(filter(json!({})), ErrorListFilterInput::default());
        assert_eq!(
            filter(json!({"warnings": false, "project": null, "text": "CS01"})),
            ErrorListFilterInput {
                warnings: Some(false),
                project: Some(None),
                text: Some("CS01".into()),
                ..Default::default()
            }
        );
        assert_eq!(filter(json!({"project": ""})).project, Some(None));
        assert_eq!(
            filter(json!({"project": "Eludite.Host"})).project,
            Some(Some("Eludite.Host".into()))
        );
        for (id, bad) in [
            (EDITOR_GO_TO_DEFINITION, json!({"target": -1})),
            (EDITOR_GO_TO_DEFINITION, json!({"target": 0, "line": 2})),
            (EDITOR_GO_TO_DEFINITION, json!({"column": 2})),
            (EDITOR_FIND_REFERENCES, json!({"line": 0})),
            (EDITOR_FIND_REFERENCES, json!({"trigger": "."})),
            (NAVIGATION_BACK, json!({"steps": 2})),
            (ERROR_LIST_FILTER, json!({"errors": "yes"})),
            (ERROR_LIST_FILTER, json!({"severity": "error"})),
        ] {
            assert!(
                matches!(parse(id, bad.clone()), Err(CommandError::InvalidInput(_))),
                "{id} {bad}"
            );
        }
        let target = DefinitionTarget {
            path: "/tmp/MetadataAsSource/1/D/2/JsonRpc.cs".into(),
            line: 31,
            column: 14,
            metadata: true,
            title: Some("JsonRpc [from metadata]".into()),
        };
        let cases = [
            (
                EDITOR_GO_TO_DEFINITION,
                WorkspaceOutput::GoToDefinition(DefinitionOutput {
                    path: "/a.cs".into(),
                    line: 4,
                    column: 9,
                    state: DefinitionState::Navigated,
                    targets: vec![target.clone()],
                    navigated: Some(target),
                    message: None,
                }),
            ),
            (
                EDITOR_FIND_REFERENCES,
                WorkspaceOutput::FindReferences(ReferencesOutput {
                    path: "/a.cs".into(),
                    line: 4,
                    column: 9,
                    state: ReferencesState::Done,
                    symbol: "HostRpcTarget".into(),
                    total: 1,
                    references: vec![ReferenceRow {
                        project: Some("Eludite.Host".into()),
                        path: "/a.cs".into(),
                        line: 4,
                        column: 9,
                        text: "class HostRpcTarget".into(),
                    }],
                    message: None,
                }),
            ),
            (
                NAVIGATION_BACK,
                WorkspaceOutput::Navigation(NavigationOutput {
                    navigated: true,
                    path: Some("/a.cs".into()),
                    line: Some(4),
                    column: Some(9),
                    back: 0,
                    forward: 1,
                }),
            ),
            (
                ERROR_LIST_FILTER,
                WorkspaceOutput::ErrorListFilter(ErrorListFilterOutput {
                    errors: false,
                    warnings: true,
                    messages: false,
                    project: Some("Eludite.Host".into()),
                    text: String::new(),
                    counts: FilterCounts {
                        errors: 2,
                        warnings: 1,
                        messages: 0,
                    },
                    shown: 1,
                    total: 5,
                }),
            ),
        ];
        for (id, out) in cases {
            conforms(&spec(id).output_schema, &out.to_json());
            assert_eq!(spec(id).permission, PermissionClass::Read, "{id}");
        }
        let states = &spec(EDITOR_GO_TO_DEFINITION).output_schema["properties"]["state"]["enum"];
        for s in [
            DefinitionState::Loading,
            DefinitionState::Navigated,
            DefinitionState::Choose,
            DefinitionState::None,
            DefinitionState::Failed,
        ] {
            assert!(
                states
                    .as_array()
                    .unwrap()
                    .contains(&serde_json::to_value(s).unwrap())
            );
        }
        assert_eq!(
            spec(EDITOR_FIND_REFERENCES).output_schema["properties"]["references"]["maxItems"],
            json!(MAX_REFERENCE_ROWS)
        );
        assert_eq!(
            spec(EDITOR_GO_TO_DEFINITION).output_schema["properties"]["targets"]["maxItems"],
            json!(MAX_DEFINITION_TARGETS)
        );
        assert!(
            WorkspaceOutput::GoToDefinition(DefinitionOutput {
                path: "/a.cs".into(),
                line: 1,
                column: 1,
                state: DefinitionState::Loading,
                targets: vec![],
                navigated: None,
                message: None,
            })
            .is_loading()
        );
    }

    #[test]
    fn register_replaces_the_placeholder_and_routes_to_the_target() {
        let mut r = crate::builtins::default_registry();
        let target = Arc::new(Recorder::default());
        register(&mut r, target.clone());
        let out = r
            .invoke(SOLUTION_OPEN, json!({"path": "/s/App.slnx"}))
            .unwrap();
        assert_eq!(out, json!({"path": "/s/App.slnx", "state": "loading"}));
        r.invoke(FILE_OPEN, json!({"path": "/a.cs"})).unwrap();
        assert!(r.invoke(FILE_OPEN, json!({})).is_err());
        assert_eq!(target.0.lock().unwrap().len(), 2);
        assert_eq!(r.lookup(FILE_OPEN).unwrap().title, "File: Open");
        assert_eq!(r.list().filter(|s| s.id.as_str() == FILE_OPEN).count(), 1);
    }
}
