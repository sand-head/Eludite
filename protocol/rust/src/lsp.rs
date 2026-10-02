//! The typed subset of LSP 3.17 that `eludite-host` forwards (host-rpc.md, "Forwarded LSP methods, typed").
//!
//! Only the members Eludite reads are typed. Every response type keeps the members it does not name in an `extra`
//! map, so a message survives a decode and re-encode unchanged. Requests carry `eluditeGeneration` on the wire; the
//! marker types here mark them [`RequestType::GENERATIONAL`] and a client adds the member (see
//! [`crate::host::WithGeneration`]).

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::host::{WithGeneration, methods};
use crate::jsonrpc::Id;
use crate::typed::{NotificationType, RequestType};

/// Zero-based line and UTF-16 code unit offset.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Position {
    pub line: u32,
    pub character: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Range {
    pub start: Position,
    pub end: Position,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Location {
    pub uri: String,
    pub range: Range,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LocationLink {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin_selection_range: Option<Range>,
    pub target_uri: String,
    pub target_range: Range,
    pub target_selection_range: Range,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TextDocumentIdentifier {
    pub uri: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VersionedTextDocumentIdentifier {
    pub uri: String,
    pub version: i32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TextDocumentItem {
    pub uri: String,
    pub language_id: String,
    pub version: i32,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DidOpenTextDocumentParams {
    pub text_document: TextDocumentItem,
}

/// A full replacement when `range` is `None`, else an incremental edit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TextDocumentContentChangeEvent {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub range: Option<Range>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub range_length: Option<u32>,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DidChangeTextDocumentParams {
    pub text_document: VersionedTextDocumentIdentifier,
    pub content_changes: Vec<TextDocumentContentChangeEvent>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DidCloseTextDocumentParams {
    pub text_document: TextDocumentIdentifier,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TextDocumentPositionParams {
    pub text_document: TextDocumentIdentifier,
    pub position: Position,
}

/// 1 = invoked, 2 = trigger character, 3 = re-trigger for incomplete results.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompletionContext {
    pub trigger_kind: u8,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trigger_character: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompletionParams {
    pub text_document: TextDocumentIdentifier,
    pub position: Position,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<CompletionContext>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompletionItem {
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    /// `string` or `MarkupContent`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub documentation: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sort_text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filter_text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub insert_text: Option<String>,
    /// `TextEdit` or `InsertReplaceEdit`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text_edit: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
    /// Members not typed above (labelDetails, commitCharacters, additionalTextEdits, ...).
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompletionList {
    pub is_incomplete: bool,
    pub items: Vec<CompletionItem>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum CompletionResponse {
    List(CompletionList),
    Items(Vec<CompletionItem>),
}

impl CompletionResponse {
    pub fn items(&self) -> &[CompletionItem] {
        match self {
            CompletionResponse::List(l) => &l.items,
            CompletionResponse::Items(i) => i,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Hover {
    /// `MarkupContent`, `MarkedString` or `MarkedString[]`.
    pub contents: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub range: Option<Range>,
}

/// 1 = invoked, 2 = trigger character, 3 = content change (the cursor moved or the text changed while it was open).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SignatureHelpContext {
    pub trigger_kind: u8,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trigger_character: Option<String>,
    pub is_retrigger: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_signature_help: Option<SignatureHelp>,
}

/// `textDocument/signatureHelp` params (schema: `protocol/schemas/host/signature-help.json`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SignatureHelpParams {
    pub text_document: TextDocumentIdentifier,
    pub position: Position,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<SignatureHelpContext>,
}

/// A parameter's label: a substring of the signature label, or `[start, end)` UTF-16 offsets into it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ParameterLabel {
    Simple(String),
    Offsets([u32; 2]),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ParameterInformation {
    pub label: ParameterLabel,
    /// `string` or `MarkupContent`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub documentation: Option<Value>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SignatureInformation {
    pub label: String,
    /// `string` or `MarkupContent`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub documentation: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parameters: Option<Vec<ParameterInformation>>,
    /// Overrides [`SignatureHelp::active_parameter`] for this signature.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_parameter: Option<u32>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SignatureHelp {
    pub signatures: Vec<SignatureInformation>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_signature: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_parameter: Option<u32>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl SignatureInformation {
    /// The `[start, end)` byte range of parameter `index` in [`SignatureInformation::label`]: a string label is
    /// searched after the opening parenthesis (then anywhere), UTF-16 offsets are converted.
    pub fn parameter_range(&self, index: usize) -> Option<std::ops::Range<usize>> {
        let label = &self.label;
        match &self.parameters.as_ref()?.get(index)?.label {
            ParameterLabel::Simple(p) if !p.is_empty() => {
                // Parameters appear in order; start after the previous parameter's match.
                let mut from = label.find('(').map_or(0, |i| i + 1);
                for prev in 0..index {
                    if let Some(r) = self.parameter_range(prev) {
                        from = from.max(r.end);
                    }
                }
                label[from..]
                    .find(p.as_str())
                    .map(|i| from + i..from + i + p.len())
                    .or_else(|| label.find(p.as_str()).map(|i| i..i + p.len()))
            }
            ParameterLabel::Simple(_) => None,
            ParameterLabel::Offsets([s, e]) => {
                let byte = |units: u32| {
                    let mut n = 0u32;
                    for (i, ch) in label.char_indices() {
                        if n >= units {
                            return Some(i);
                        }
                        n += ch.len_utf16() as u32;
                    }
                    (n >= units).then_some(label.len())
                };
                let (s, e) = (byte(*s)?, byte(*e)?);
                (s <= e).then_some(s..e)
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum DefinitionResponse {
    Scalar(Location),
    Array(Vec<Location>),
    Links(Vec<LocationLink>),
}

impl DefinitionResponse {
    /// The targets as plain locations, in the server's order. A `LocationLink` becomes its `targetUri` with its
    /// `targetSelectionRange` (the name, where the caret goes), as `textDocument/definition` clients use it.
    pub fn into_locations(self) -> Vec<Location> {
        match self {
            DefinitionResponse::Scalar(l) => vec![l],
            DefinitionResponse::Array(ls) => ls,
            DefinitionResponse::Links(links) => links
                .into_iter()
                .map(|l| Location {
                    uri: l.target_uri,
                    range: l.target_selection_range,
                })
                .collect(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReferenceContext {
    pub include_declaration: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReferenceParams {
    pub text_document: TextDocumentIdentifier,
    pub position: Position,
    pub context: ReferenceContext,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DocumentSymbolParams {
    pub text_document: TextDocumentIdentifier,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DocumentSymbol {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    pub kind: u32,
    pub range: Range,
    pub selection_range: Range,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub children: Option<Vec<DocumentSymbol>>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SymbolInformation {
    pub name: String,
    pub kind: u32,
    pub location: Location,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub container_name: Option<String>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum DocumentSymbolResponse {
    Nested(Vec<DocumentSymbol>),
    Flat(Vec<SymbolInformation>),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceSymbolParams {
    pub query: String,
}

/// LSP 3.17 `WorkspaceSymbol`: `location` may be a `Location` or `{ uri }` (resolved later).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceSymbol {
    pub name: String,
    pub kind: u32,
    pub location: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub container_name: Option<String>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum WorkspaceSymbolResponse {
    Flat(Vec<SymbolInformation>),
    Nested(Vec<WorkspaceSymbol>),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Diagnostic {
    pub range: Range,
    /// 1 error, 2 warning, 3 information, 4 hint.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub severity: Option<u8>,
    /// Integer or string.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    pub message: String,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DocumentDiagnosticParams {
    pub text_document: TextDocumentIdentifier,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identifier: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub previous_result_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum DocumentDiagnosticReport {
    #[serde(rename_all = "camelCase")]
    Full {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        result_id: Option<String>,
        items: Vec<Diagnostic>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        related_documents: Option<Value>,
    },
    #[serde(rename_all = "camelCase")]
    Unchanged {
        result_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        related_documents: Option<Value>,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PublishDiagnosticsParams {
    pub uri: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<i32>,
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CancelParams {
    pub id: Id,
}

/// LSP `TextEdit`; an `AnnotatedTextEdit`'s `annotationId` stays in `extra`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TextEdit {
    pub range: Range,
    pub new_text: String,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl TextEdit {
    pub fn new(range: Range, new_text: impl Into<String>) -> Self {
        Self {
            range,
            new_text: new_text.into(),
            extra: Map::new(),
        }
    }
}

/// `{ uri, version: integer | null }`: `null` means "whatever the client has" (the pinned Roslyn always sends it).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OptionalVersionedTextDocumentIdentifier {
    pub uri: String,
    #[serde(default)]
    pub version: Option<i32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TextDocumentEdit {
    pub text_document: OptionalVersionedTextDocumentIdentifier,
    pub edits: Vec<TextEdit>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateFileOptions {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub overwrite: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ignore_if_exists: Option<bool>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeleteFileOptions {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recursive: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ignore_if_not_exists: Option<bool>,
}

/// `CreateFile`, `RenameFile` or `DeleteFile` (by `kind`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum ResourceOperation {
    #[serde(rename_all = "camelCase")]
    Create {
        uri: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        options: Option<CreateFileOptions>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        annotation_id: Option<String>,
    },
    #[serde(rename_all = "camelCase")]
    Rename {
        old_uri: String,
        new_uri: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        options: Option<CreateFileOptions>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        annotation_id: Option<String>,
    },
    #[serde(rename_all = "camelCase")]
    Delete {
        uri: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        options: Option<DeleteFileOptions>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        annotation_id: Option<String>,
    },
}

/// One entry of `WorkspaceEdit.documentChanges`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum DocumentChange {
    Edit(TextDocumentEdit),
    Operation(ResourceOperation),
}

/// LSP 3.17 `WorkspaceEdit` (schema: `protocol/schemas/host/apply-edit.json` `$defs.workspaceEdit`). When both are
/// present, `document_changes` wins (the LSP rule).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceEdit {
    /// Document URI to edits (no versions).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub changes: Option<std::collections::BTreeMap<String, Vec<TextEdit>>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub document_changes: Option<Vec<DocumentChange>>,
    /// `changeAnnotations` and anything else.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// `textDocument/prepareRename`'s answer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum PrepareRenameResponse {
    Range(Range),
    RangeWithPlaceholder {
        range: Range,
        placeholder: String,
    },
    #[serde(rename_all = "camelCase")]
    DefaultBehavior {
        default_behavior: bool,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RenameParams {
    pub text_document: TextDocumentIdentifier,
    pub position: Position,
    pub new_name: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CodeActionContext {
    pub diagnostics: Vec<Diagnostic>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub only: Option<Vec<String>>,
    /// 1 = invoked, 2 = automatic.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trigger_kind: Option<u8>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CodeActionParams {
    pub text_document: TextDocumentIdentifier,
    pub range: Range,
    pub context: CodeActionContext,
}

/// LSP `Command`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Command {
    pub title: String,
    pub command: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub arguments: Option<Vec<Value>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodeActionDisabled {
    pub reason: String,
}

/// LSP `CodeAction` (schema: `protocol/schemas/host/code-action.json` `$defs.codeAction`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CodeAction {
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diagnostics: Option<Vec<Diagnostic>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub is_preferred: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disabled: Option<CodeActionDisabled>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub edit: Option<WorkspaceEdit>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<Command>,
    /// Opaque; sent back unchanged in `codeAction/resolve`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// One entry of the `textDocument/codeAction` answer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum CodeActionOrCommand {
    Command(Command),
    Action(Box<CodeAction>),
}

/// `workspace/applyEdit` params (the host adds `eluditeGeneration`, see [`ApplyEdit`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApplyWorkspaceEditParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    pub edit: WorkspaceEdit,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApplyWorkspaceEditResult {
    pub applied: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure_reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failed_change: Option<u32>,
}

macro_rules! forwarded_request {
    ($(#[$m:meta])* $name:ident, $method:expr, $params:ty, $result:ty) => {
        $(#[$m])*
        #[derive(Debug)]
        pub enum $name {}
        impl RequestType for $name {
            const METHOD: &'static str = $method;
            const GENERATIONAL: bool = true;
            type Params = $params;
            type Result = $result;
        }
    };
}

macro_rules! notification {
    ($(#[$m:meta])* $name:ident, $method:expr, $params:ty) => {
        $(#[$m])*
        #[derive(Debug)]
        pub enum $name {}
        impl NotificationType for $name {
            const METHOD: &'static str = $method;
            type Params = $params;
        }
    };
}

forwarded_request!(
    /// `textDocument/completion`.
    Completion,
    "textDocument/completion",
    CompletionParams,
    Option<CompletionResponse>
);
forwarded_request!(
    /// `completionItem/resolve`.
    ResolveCompletionItem,
    "completionItem/resolve",
    CompletionItem,
    CompletionItem
);
forwarded_request!(
    /// `textDocument/hover`.
    HoverRequest,
    "textDocument/hover",
    TextDocumentPositionParams,
    Option<Hover>
);
forwarded_request!(
    /// `textDocument/signatureHelp`.
    SignatureHelpRequest,
    "textDocument/signatureHelp",
    SignatureHelpParams,
    Option<SignatureHelp>
);
forwarded_request!(
    /// `textDocument/definition`.
    GotoDefinition,
    "textDocument/definition",
    TextDocumentPositionParams,
    Option<DefinitionResponse>
);
forwarded_request!(
    /// `textDocument/references`.
    References,
    "textDocument/references",
    ReferenceParams,
    Option<Vec<Location>>
);
forwarded_request!(
    /// `textDocument/documentSymbol`.
    DocumentSymbolRequest,
    "textDocument/documentSymbol",
    DocumentSymbolParams,
    Option<DocumentSymbolResponse>
);
forwarded_request!(
    /// `workspace/symbol`.
    WorkspaceSymbolRequest,
    "workspace/symbol",
    WorkspaceSymbolParams,
    Option<WorkspaceSymbolResponse>
);
forwarded_request!(
    /// `textDocument/diagnostic` (pull).
    DocumentDiagnosticRequest,
    "textDocument/diagnostic",
    DocumentDiagnosticParams,
    DocumentDiagnosticReport
);

forwarded_request!(
    /// `textDocument/prepareRename`.
    PrepareRename,
    "textDocument/prepareRename",
    TextDocumentPositionParams,
    Option<PrepareRenameResponse>
);
forwarded_request!(
    /// `textDocument/rename`.
    Rename,
    "textDocument/rename",
    RenameParams,
    Option<WorkspaceEdit>
);
forwarded_request!(
    /// `textDocument/codeAction`.
    CodeActionRequest,
    "textDocument/codeAction",
    CodeActionParams,
    Option<Vec<CodeActionOrCommand>>
);
forwarded_request!(
    /// `codeAction/resolve`.
    ResolveCodeAction,
    "codeAction/resolve",
    CodeAction,
    CodeAction
);

/// `workspace/applyEdit`, the request the host sends the shell (host-rpc.md, "Messages the host sends"). Not
/// generational in the forwarded sense: the host adds `eluditeGeneration` itself, and the shell answers it.
#[derive(Debug)]
pub enum ApplyEdit {}
impl RequestType for ApplyEdit {
    const METHOD: &'static str = methods::APPLY_EDIT;
    const GENERATIONAL: bool = false;
    type Params = WithGeneration<ApplyWorkspaceEditParams>;
    type Result = ApplyWorkspaceEditResult;
}

notification!(
    /// `textDocument/didOpen`.
    DidOpenTextDocument,
    "textDocument/didOpen",
    DidOpenTextDocumentParams
);
notification!(
    /// `textDocument/didChange`.
    DidChangeTextDocument,
    "textDocument/didChange",
    DidChangeTextDocumentParams
);
notification!(
    /// `textDocument/didClose`.
    DidCloseTextDocument,
    "textDocument/didClose",
    DidCloseTextDocumentParams
);
notification!(
    /// `$/cancelRequest`.
    Cancel,
    methods::CANCEL_REQUEST,
    CancelParams
);
notification!(
    /// `textDocument/publishDiagnostics` from the host: LSP params plus the generation the pull ran under.
    PublishDiagnostics,
    methods::PUBLISH_DIAGNOSTICS,
    WithGeneration<PublishDiagnosticsParams>
);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::round_trip;
    use serde_json::json;

    fn pos(line: u32, character: u32) -> Position {
        Position { line, character }
    }

    fn range() -> Range {
        Range {
            start: pos(1, 2),
            end: pos(1, 9),
        }
    }

    fn doc() -> TextDocumentIdentifier {
        TextDocumentIdentifier {
            uri: "file:///a.cs".into(),
        }
    }

    #[test]
    fn text_sync() {
        round_trip(
            &DidOpenTextDocumentParams {
                text_document: TextDocumentItem {
                    uri: "file:///a.cs".into(),
                    language_id: "csharp".into(),
                    version: 1,
                    text: "class A {}".into(),
                },
            },
            json!({"textDocument": {"uri": "file:///a.cs", "languageId": "csharp", "version": 1, "text": "class A {}"}}),
        );
        round_trip(
            &DidChangeTextDocumentParams {
                text_document: VersionedTextDocumentIdentifier {
                    uri: "file:///a.cs".into(),
                    version: 2,
                },
                content_changes: vec![
                    TextDocumentContentChangeEvent {
                        range: Some(range()),
                        range_length: None,
                        text: "x".into(),
                    },
                    TextDocumentContentChangeEvent {
                        range: None,
                        range_length: None,
                        text: "full".into(),
                    },
                ],
            },
            json!({"textDocument": {"uri": "file:///a.cs", "version": 2}, "contentChanges": [
                {"range": {"start": {"line": 1, "character": 2}, "end": {"line": 1, "character": 9}}, "text": "x"},
                {"text": "full"}
            ]}),
        );
        round_trip(
            &DidCloseTextDocumentParams {
                text_document: doc(),
            },
            json!({"textDocument": {"uri": "file:///a.cs"}}),
        );
    }

    #[test]
    fn completion_request_with_generation() {
        round_trip(
            &WithGeneration {
                params: CompletionParams {
                    text_document: doc(),
                    position: pos(3, 7),
                    context: Some(CompletionContext {
                        trigger_kind: 2,
                        trigger_character: Some(".".into()),
                    }),
                },
                generation: 1,
            },
            json!({"textDocument": {"uri": "file:///a.cs"}, "position": {"line": 3, "character": 7},
                   "context": {"triggerKind": 2, "triggerCharacter": "."}, "eluditeGeneration": 1}),
        );
    }

    #[test]
    fn completion_responses() {
        let list = json!({"isIncomplete": false, "itemDefaults": {"editRange": null},
            "items": [{"label": "Compute", "kind": 2, "sortText": "a", "data": {"id": 1}, "labelDetails": {"detail": "()"}}]});
        let parsed: Option<CompletionResponse> = serde_json::from_value(list.clone()).unwrap();
        let parsed = parsed.unwrap();
        assert!(matches!(parsed, CompletionResponse::List(_)));
        assert_eq!(parsed.items()[0].label, "Compute");
        assert_eq!(parsed.items()[0].extra["labelDetails"]["detail"], "()");
        round_trip(&Some(parsed), list);

        let items = json!([{"label": "a"}, {"label": "b", "documentation": {"kind": "markdown", "value": "x"}}]);
        let parsed: CompletionResponse = serde_json::from_value(items.clone()).unwrap();
        assert!(matches!(parsed, CompletionResponse::Items(ref v) if v.len() == 2));
        round_trip(&parsed, items);
        round_trip(&None::<CompletionResponse>, Value::Null);
    }

    #[test]
    fn resolve_item_with_generation() {
        round_trip(
            &WithGeneration {
                params: CompletionItem {
                    label: "Compute".into(),
                    data: Some(json!({"k": 1})),
                    ..Default::default()
                },
                generation: 2,
            },
            json!({"label": "Compute", "data": {"k": 1}, "eluditeGeneration": 2}),
        );
    }

    #[test]
    fn hover() {
        round_trip(
            &WithGeneration {
                params: TextDocumentPositionParams {
                    text_document: doc(),
                    position: pos(0, 1),
                },
                generation: 0,
            },
            json!({"textDocument": {"uri": "file:///a.cs"}, "position": {"line": 0, "character": 1}, "eluditeGeneration": 0}),
        );
        round_trip(
            &Some(Hover {
                contents: json!({"kind": "markdown", "value": "int x"}),
                range: Some(range()),
            }),
            json!({"contents": {"kind": "markdown", "value": "int x"},
                   "range": {"start": {"line": 1, "character": 2}, "end": {"line": 1, "character": 9}}}),
        );
    }

    #[test]
    fn signature_help() {
        round_trip(
            &WithGeneration {
                params: SignatureHelpParams {
                    text_document: doc(),
                    position: pos(4, 18),
                    context: Some(SignatureHelpContext {
                        trigger_kind: 2,
                        trigger_character: Some(",".into()),
                        is_retrigger: true,
                        active_signature_help: None,
                    }),
                },
                generation: 3,
            },
            json!({"textDocument": {"uri": "file:///a.cs"}, "position": {"line": 4, "character": 18},
                   "context": {"triggerKind": 2, "triggerCharacter": ",", "isRetrigger": true},
                   "eluditeGeneration": 3}),
        );
        let roslyn = json!({"signatures": [
            {"label": "void Console.WriteLine(string format, object? arg0)",
             "documentation": {"kind": "plaintext", "value": "Writes."},
             "parameters": [{"label": "string format", "documentation": "The format."},
                            {"label": "object? arg0"}]},
            {"label": "void M(int a, int b)", "parameters": [{"label": [7, 12]}, {"label": [14, 19]}],
             "activeParameter": 1}],
            "activeSignature": 0, "activeParameter": 1});
        let parsed: Option<SignatureHelp> = serde_json::from_value(roslyn.clone()).unwrap();
        let help = parsed.clone().unwrap();
        assert_eq!(help.active_parameter, Some(1));
        let s0 = &help.signatures[0];
        assert_eq!(&s0.label[s0.parameter_range(0).unwrap()], "string format");
        assert_eq!(&s0.label[s0.parameter_range(1).unwrap()], "object? arg0");
        let s1 = &help.signatures[1];
        assert_eq!(&s1.label[s1.parameter_range(0).unwrap()], "int a");
        assert_eq!(&s1.label[s1.parameter_range(1).unwrap()], "int b");
        assert_eq!(s1.parameter_range(2), None);
        assert_eq!(s1.active_parameter, Some(1));
        round_trip(&parsed, roslyn);
        round_trip(&None::<SignatureHelp>, Value::Null);
        // Repeated parameter text resolves to successive occurrences.
        let twice = SignatureInformation {
            label: "M(int x, int x)".into(),
            documentation: None,
            parameters: Some(vec![
                ParameterInformation {
                    label: ParameterLabel::Simple("int x".into()),
                    documentation: None,
                    extra: Map::new(),
                },
                ParameterInformation {
                    label: ParameterLabel::Simple("int x".into()),
                    documentation: None,
                    extra: Map::new(),
                },
            ]),
            active_parameter: None,
            extra: Map::new(),
        };
        assert_eq!(twice.parameter_range(0), Some(2..7));
        assert_eq!(twice.parameter_range(1), Some(9..14));
    }

    #[test]
    fn definition_shapes() {
        let loc = json!({"uri": "file:///b.cs", "range": {"start": {"line": 1, "character": 2}, "end": {"line": 1, "character": 9}}});
        let one: DefinitionResponse = serde_json::from_value(loc.clone()).unwrap();
        assert!(matches!(one, DefinitionResponse::Scalar(_)));
        round_trip(&one, loc.clone());
        let many: DefinitionResponse = serde_json::from_value(json!([loc])).unwrap();
        assert!(matches!(many, DefinitionResponse::Array(_)));
        let r = range();
        let links = DefinitionResponse::Links(vec![LocationLink {
            origin_selection_range: None,
            target_uri: "file:///b.cs".into(),
            target_range: r,
            target_selection_range: r,
        }]);
        let v = serde_json::to_value(&links).unwrap();
        assert_eq!(v[0]["targetUri"], "file:///b.cs");
        round_trip(&links, v);
    }

    #[test]
    fn definition_results_flatten_to_locations() {
        // The pinned Roslyn's answer for a type in a referenced assembly (brief 0014, host-rpc.md "Metadata as
        // source"): a plain file URI to its decompiled text under the temporary directory.
        let roslyn: DefinitionResponse = serde_json::from_value(json!([{
            "uri": "file:///tmp/MetadataAsSource/006fad54/DecompilationMetadataAsSourceFileProvider/c19c8186/JsonRpc.cs",
            "range": {"start": {"line": 30, "character": 13}, "end": {"line": 30, "character": 20}}}]))
        .unwrap();
        let locations = roslyn.into_locations();
        assert_eq!(locations.len(), 1);
        assert!(locations[0].uri.ends_with("/JsonRpc.cs"));
        assert_eq!(locations[0].range.start, pos(30, 13));
        let one: DefinitionResponse = serde_json::from_value(
            json!({"uri": "file:///b.cs", "range": {"start": {"line": 1, "character": 2}, "end": {"line": 1, "character": 9}}}),
        )
        .unwrap();
        assert_eq!(one.into_locations()[0].uri, "file:///b.cs");
        let name = Range {
            start: pos(4, 6),
            end: pos(4, 9),
        };
        let links = DefinitionResponse::Links(vec![LocationLink {
            origin_selection_range: None,
            target_uri: "file:///c.cs".into(),
            target_range: range(),
            target_selection_range: name,
        }]);
        assert_eq!(
            links.into_locations(),
            [Location {
                uri: "file:///c.cs".into(),
                range: name
            }]
        );
        let none: DefinitionResponse = serde_json::from_value(json!([])).unwrap();
        assert!(none.into_locations().is_empty());
    }

    #[test]
    fn references() {
        round_trip(
            &ReferenceParams {
                text_document: doc(),
                position: pos(2, 3),
                context: ReferenceContext {
                    include_declaration: true,
                },
            },
            json!({"textDocument": {"uri": "file:///a.cs"}, "position": {"line": 2, "character": 3},
                   "context": {"includeDeclaration": true}}),
        );
        round_trip(
            &Some(vec![Location {
                uri: "file:///a.cs".into(),
                range: range(),
            }]),
            json!([{"uri": "file:///a.cs", "range": {"start": {"line": 1, "character": 2}, "end": {"line": 1, "character": 9}}}]),
        );
    }

    #[test]
    fn document_symbols() {
        round_trip(
            &DocumentSymbolParams {
                text_document: doc(),
            },
            json!({"textDocument": {"uri": "file:///a.cs"}}),
        );
        let r = json!({"start": {"line": 0, "character": 0}, "end": {"line": 9, "character": 1}});
        let nested = json!([{"name": "A", "kind": 5, "range": r, "selectionRange": r,
                             "children": [{"name": "M", "kind": 6, "detail": "void M()", "range": r, "selectionRange": r}]}]);
        let parsed: DocumentSymbolResponse = serde_json::from_value(nested.clone()).unwrap();
        assert!(
            matches!(parsed, DocumentSymbolResponse::Nested(ref v) if v[0].children.as_ref().unwrap()[0].name == "M")
        );
        round_trip(&parsed, nested);
        let flat = json!([{"name": "A", "kind": 5, "location": {"uri": "file:///a.cs", "range": r}, "containerName": "N"}]);
        let parsed: DocumentSymbolResponse = serde_json::from_value(flat.clone()).unwrap();
        assert!(matches!(parsed, DocumentSymbolResponse::Flat(_)));
        round_trip(&parsed, flat);
    }

    #[test]
    fn workspace_symbols() {
        round_trip(
            &WithGeneration {
                params: WorkspaceSymbolParams {
                    query: "Widget".into(),
                },
                generation: 5,
            },
            json!({"query": "Widget", "eluditeGeneration": 5}),
        );
        let r = json!({"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 5}});
        let flat = json!([{"name": "Widget00", "kind": 5, "location": {"uri": "file:///w.cs", "range": r}}]);
        let parsed: WorkspaceSymbolResponse = serde_json::from_value(flat.clone()).unwrap();
        assert!(matches!(parsed, WorkspaceSymbolResponse::Flat(_)));
        round_trip(&parsed, flat);
        let nested = json!([{"name": "Widget00", "kind": 5, "location": {"uri": "file:///w.cs"}, "data": 1}]);
        let parsed: WorkspaceSymbolResponse = serde_json::from_value(nested.clone()).unwrap();
        assert!(matches!(parsed, WorkspaceSymbolResponse::Nested(_)));
        round_trip(&parsed, nested);
    }

    #[test]
    fn pull_diagnostics() {
        round_trip(
            &DocumentDiagnosticParams {
                text_document: doc(),
                identifier: None,
                previous_result_id: Some("r1".into()),
            },
            json!({"textDocument": {"uri": "file:///a.cs"}, "previousResultId": "r1"}),
        );
        let full = json!({"kind": "full", "resultId": "r2", "items": [
            {"range": {"start": {"line": 1, "character": 2}, "end": {"line": 1, "character": 9}},
             "severity": 1, "code": "CS0103", "source": "csharp", "message": "nope", "tags": [1]}]});
        let parsed: DocumentDiagnosticReport = serde_json::from_value(full.clone()).unwrap();
        assert!(
            matches!(parsed, DocumentDiagnosticReport::Full { ref items, .. } if items[0].extra["tags"] == json!([1]))
        );
        round_trip(&parsed, full);
        round_trip(
            &DocumentDiagnosticReport::Unchanged {
                result_id: "r2".into(),
                related_documents: None,
            },
            json!({"kind": "unchanged", "resultId": "r2"}),
        );
    }

    #[test]
    fn publish_diagnostics_from_host() {
        round_trip(
            &WithGeneration {
                params: PublishDiagnosticsParams {
                    uri: "file:///a.cs".into(),
                    version: Some(3),
                    diagnostics: vec![Diagnostic {
                        range: range(),
                        severity: Some(2),
                        code: Some(json!("CS0168")),
                        source: None,
                        message: "unused".into(),
                        extra: Map::new(),
                    }],
                },
                generation: 1,
            },
            json!({"uri": "file:///a.cs", "version": 3, "eluditeGeneration": 1, "diagnostics": [
                {"range": {"start": {"line": 1, "character": 2}, "end": {"line": 1, "character": 9}},
                 "severity": 2, "code": "CS0168", "message": "unused"}]}),
        );
    }

    #[test]
    fn workspace_edits_with_changes_document_changes_and_resource_operations() {
        // The pinned Roslyn's rename answer: documentChanges, version null.
        let roslyn = json!({"documentChanges": [
            {"textDocument": {"uri": "file:///s/A.cs", "version": null},
             "edits": [{"range": {"start": {"line": 1, "character": 2}, "end": {"line": 1, "character": 9}}, "newText": "Pong"}]},
            {"kind": "create", "uri": "file:///s/New.cs", "options": {"ignoreIfExists": true}},
            {"kind": "rename", "oldUri": "file:///s/Old.cs", "newUri": "file:///s/Renamed.cs", "options": {"overwrite": false}},
            {"kind": "delete", "uri": "file:///s/Gone", "options": {"recursive": true}},
            {"textDocument": {"uri": "file:///s/B.cs", "version": 4},
             "edits": [{"range": {"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 0}}, "newText": "using X;\n", "annotationId": "a"}]}
        ], "changeAnnotations": {"a": {"label": "imports"}}});
        let edit: WorkspaceEdit = serde_json::from_value(roslyn.clone()).unwrap();
        let changes = edit.document_changes.as_ref().unwrap();
        assert!(
            matches!(&changes[0], DocumentChange::Edit(e) if e.text_document.version.is_none() && e.edits[0].new_text == "Pong")
        );
        assert!(matches!(
            &changes[1],
            DocumentChange::Operation(ResourceOperation::Create {
                options: Some(CreateFileOptions {
                    ignore_if_exists: Some(true),
                    ..
                }),
                ..
            })
        ));
        assert!(matches!(
            &changes[2],
            DocumentChange::Operation(ResourceOperation::Rename { old_uri, new_uri, .. }) if old_uri.ends_with("Old.cs") && new_uri.ends_with("Renamed.cs")
        ));
        assert!(matches!(
            &changes[3],
            DocumentChange::Operation(ResourceOperation::Delete {
                options: Some(DeleteFileOptions {
                    recursive: Some(true),
                    ..
                }),
                ..
            })
        ));
        assert!(
            matches!(&changes[4], DocumentChange::Edit(e) if e.text_document.version == Some(4) && e.edits[0].extra["annotationId"] == "a")
        );
        assert_eq!(edit.extra["changeAnnotations"]["a"]["label"], "imports");
        round_trip(&edit, roslyn);
        let plain = json!({"changes": {"file:///s/A.cs": [
            {"range": {"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 1}}, "newText": "x"}]}});
        let edit: WorkspaceEdit = serde_json::from_value(plain.clone()).unwrap();
        assert_eq!(edit.changes.as_ref().unwrap()["file:///s/A.cs"].len(), 1);
        round_trip(&edit, plain);
    }

    #[test]
    fn rename_and_prepare_rename() {
        round_trip(
            &WithGeneration {
                params: RenameParams {
                    text_document: doc(),
                    position: pos(4, 18),
                    new_name: "Pong".into(),
                },
                generation: 2,
            },
            json!({"textDocument": {"uri": "file:///a.cs"}, "position": {"line": 4, "character": 18},
                   "newName": "Pong", "eluditeGeneration": 2}),
        );
        let r = json!({"start": {"line": 1, "character": 2}, "end": {"line": 1, "character": 9}});
        let p: Option<PrepareRenameResponse> = serde_json::from_value(r.clone()).unwrap();
        assert_eq!(p, Some(PrepareRenameResponse::Range(range())));
        round_trip(&p, r.clone());
        let with = json!({"range": r, "placeholder": "Ping"});
        let p: PrepareRenameResponse = serde_json::from_value(with.clone()).unwrap();
        assert!(
            matches!(p, PrepareRenameResponse::RangeWithPlaceholder { ref placeholder, .. } if placeholder == "Ping")
        );
        round_trip(&p, with);
        let default = json!({"defaultBehavior": true});
        let p: PrepareRenameResponse = serde_json::from_value(default.clone()).unwrap();
        assert_eq!(
            p,
            PrepareRenameResponse::DefaultBehavior {
                default_behavior: true
            }
        );
        round_trip(&p, default);
        round_trip(&None::<PrepareRenameResponse>, Value::Null);
        round_trip(&None::<WorkspaceEdit>, Value::Null);
    }

    #[test]
    fn code_actions_as_roslyn_sends_them() {
        round_trip(
            &WithGeneration {
                params: CodeActionParams {
                    text_document: doc(),
                    range: range(),
                    context: CodeActionContext {
                        diagnostics: vec![],
                        only: None,
                        trigger_kind: Some(2),
                    },
                },
                generation: 1,
            },
            json!({"textDocument": {"uri": "file:///a.cs"},
                   "range": {"start": {"line": 1, "character": 2}, "end": {"line": 1, "character": 9}},
                   "context": {"diagnostics": [], "triggerKind": 2}, "eluditeGeneration": 1}),
        );
        let data = json!({"UniqueIdentifier": "Use primary constructor", "CustomTags": [], "CodeActionPath": ["Use primary constructor"]});
        let answer = json!([
            {"title": "Use primary constructor", "kind": "quickfix", "diagnostics": [], "data": data},
            {"title": "Fix All: Use primary constructor", "kind": "quickfix",
             "command": {"title": "Fix All: Use primary constructor", "command": "roslyn.client.fixAllCodeAction", "arguments": [data]},
             "data": data},
            {"title": "Generate parameter", "kind": "refactor",
             "command": {"title": "Generate parameter", "command": "roslyn.client.nestedCodeAction",
                         "arguments": [{"NestedCodeActions": [{"title": "Generate parameter 'x'", "data": data}]}]},
             "data": data},
            {"title": "Organize", "command": "x.organize"}
        ]);
        let parsed: Option<Vec<CodeActionOrCommand>> =
            serde_json::from_value(answer.clone()).unwrap();
        let parsed = parsed.unwrap();
        assert!(
            matches!(&parsed[0], CodeActionOrCommand::Action(a) if a.edit.is_none() && a.data.is_some() && a.kind.as_deref() == Some("quickfix"))
        );
        assert!(
            matches!(&parsed[1], CodeActionOrCommand::Action(a) if a.command.as_ref().unwrap().command == "roslyn.client.fixAllCodeAction")
        );
        assert!(
            matches!(&parsed[2], CodeActionOrCommand::Action(a) if a.command.as_ref().unwrap().arguments.as_ref().unwrap()[0]["NestedCodeActions"][0]["title"] == "Generate parameter 'x'")
        );
        assert!(matches!(&parsed[3], CodeActionOrCommand::Command(c) if c.command == "x.organize"));
        round_trip(&Some(parsed), answer);
        let resolved = json!({"title": "Use primary constructor", "kind": "quickfix", "data": data,
            "edit": {"documentChanges": [{"textDocument": {"uri": "file:///a.cs", "version": null}, "edits": []}]}});
        let a: CodeAction = serde_json::from_value(resolved.clone()).unwrap();
        assert!(a.edit.is_some());
        round_trip(&a, resolved);
    }

    #[test]
    fn apply_edit_from_the_host() {
        use crate::typed::RequestType;
        assert_eq!(ApplyEdit::METHOD, "workspace/applyEdit");
        const { assert!(!ApplyEdit::GENERATIONAL) };
        round_trip(
            &WithGeneration {
                params: ApplyWorkspaceEditParams {
                    label: Some("Rename".into()),
                    edit: WorkspaceEdit::default(),
                },
                generation: 3,
            },
            json!({"label": "Rename", "edit": {}, "eluditeGeneration": 3}),
        );
        round_trip(
            &ApplyWorkspaceEditResult {
                applied: false,
                failure_reason: Some("stale".into()),
                failed_change: Some(1),
            },
            json!({"applied": false, "failureReason": "stale", "failedChange": 1}),
        );
    }

    #[test]
    fn cancel() {
        round_trip(&CancelParams { id: Id::Number(7) }, json!({"id": 7}));
        round_trip(
            &CancelParams {
                id: Id::String("x".into()),
            },
            json!({"id": "x"}),
        );
    }
}
