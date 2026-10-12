//! Inlay hints (brief 0066): what a server's `textDocument/inlayHint` answer shows.
//!
//! [`label`] is a hint's text (a string, or its label parts joined), [`HintKind`] its kind (LSP's 1 type, 2
//! parameter; none for the rest), and [`tooltip`] the text of its tooltip (a string, `MarkupContent`, or the parts'
//! tooltips) once `inlayHint/resolve` gave it one. A hint's padding is drawn by the editor, not put into the label.

use eludite_protocol::lsp::{InlayHint, InlayHintLabel};
use serde_json::Value;

/// What a hint stands for: the settings `editor.inlayHints.types` and `editor.inlayHints.parameterNames` choose by
/// it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HintKind {
    /// LSP kind 1: an inferred type (`: int` after `var x`).
    Type,
    /// LSP kind 2: a parameter name (`count:` before an argument).
    Parameter,
    /// No kind (rust-analyzer's chaining and closing-brace hints, F#'s).
    Other,
}

impl HintKind {
    pub fn of(hint: &InlayHint) -> Self {
        match hint.kind {
            Some(1) => HintKind::Type,
            Some(2) => HintKind::Parameter,
            _ => HintKind::Other,
        }
    }
}

/// The hint's text: the label, or its parts' values joined. Line breaks become spaces (a hint is one line).
pub fn label(hint: &InlayHint) -> String {
    let text = match &hint.label {
        InlayHintLabel::String(s) => s.clone(),
        InlayHintLabel::Parts(parts) => parts.iter().map(|p| p.value.as_str()).collect(),
    };
    text.replace(['\r', '\n'], " ")
}

/// The text of a `tooltip` value: a string, or `MarkupContent`'s `value`.
fn tooltip_text(v: &Value) -> Option<String> {
    match v {
        Value::String(s) => Some(s.clone()),
        Value::Object(o) => o.get("value").and_then(Value::as_str).map(str::to_owned),
        _ => None,
    }
    .filter(|s| !s.trim().is_empty())
}

/// The hint's tooltip: its own, else its label parts' joined by blank lines; `None` when it has none (resolve it).
pub fn tooltip(hint: &InlayHint) -> Option<String> {
    if let Some(t) = hint.tooltip.as_ref().and_then(tooltip_text) {
        return Some(t);
    }
    let InlayHintLabel::Parts(parts) = &hint.label else {
        return None;
    };
    let tips: Vec<String> = parts
        .iter()
        .filter_map(|p| p.tooltip.as_ref().and_then(tooltip_text))
        .collect();
    (!tips.is_empty()).then(|| tips.join("\n\n"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn hint(v: Value) -> InlayHint {
        serde_json::from_value(v).unwrap()
    }

    #[test]
    fn labels_kinds_and_tooltips() {
        let p = hint(
            json!({"position": {"line": 0, "character": 4}, "label": "count:", "kind": 2,
                            "paddingRight": true}),
        );
        assert_eq!(label(&p), "count:");
        assert_eq!(HintKind::of(&p), HintKind::Parameter);
        assert_eq!(tooltip(&p), None);
        let t = hint(json!({"position": {"line": 0, "character": 4},
                            "label": [{"value": ": "}, {"value": "List<int>", "tooltip": "System.Collections.Generic.List<T>"}],
                            "kind": 1}));
        assert_eq!(label(&t), ": List<int>");
        assert_eq!(HintKind::of(&t), HintKind::Type);
        assert_eq!(
            tooltip(&t).as_deref(),
            Some("System.Collections.Generic.List<T>")
        );
        let r = hint(
            json!({"position": {"line": 0, "character": 4}, "label": "a\nb",
                            "tooltip": {"kind": "markdown", "value": "```csharp\nint x\n```"}}),
        );
        assert_eq!(label(&r), "a b");
        assert_eq!(HintKind::of(&r), HintKind::Other);
        assert_eq!(tooltip(&r).as_deref(), Some("```csharp\nint x\n```"));
    }
}
