//! `diagnostics.list`: read the Error List (PLAN.md 5.1, 5.4).
//!
//! The schemas are the files in `protocol/schemas/` (checked in first,
//! CLAUDE.md invariant 4), embedded at compile time so the command bus, the MCP
//! server and the tests all use the same bytes. The caller supplies the Error
//! List through a [`DiagnosticSource`]: the shell's Error List (brief 0012), or
//! [`fixture`] in brief 0005's tests.

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{CommandError, CommandId, CommandRegistry, CommandSpec, PermissionClass};

/// Command id. Exposed over MCP as the tool `diagnostics-list` (see `eludite-mcp`).
pub const DIAGNOSTICS_LIST: &str = "diagnostics.list";

/// `protocol/schemas/diagnostics-list.input.json`.
pub const INPUT_SCHEMA: &str =
    include_str!("../../../protocol/schemas/diagnostics-list.input.json");
/// `protocol/schemas/diagnostics-list.output.json`.
pub const OUTPUT_SCHEMA: &str =
    include_str!("../../../protocol/schemas/diagnostics-list.output.json");

/// Error List severities, named as in Visual Studio (Errors, Warnings, Messages).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Error,
    Warning,
    Message,
}

/// One Error List row. Line and column are 1-based; `path` is relative to the
/// workspace root with forward slashes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Diagnostic {
    pub path: String,
    pub line: u32,
    pub column: u32,
    pub severity: Severity,
    pub code: String,
    pub message: String,
    /// The project the file belongs to, as Workspace names it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
}

/// Where the command reads the current Error List from. Called on whatever
/// thread invokes the command, so it must be cheap and thread-safe.
pub type DiagnosticSource = Arc<dyn Fn() -> Vec<Diagnostic> + Send + Sync>;

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct Input {
    severity: Option<Severity>,
}

fn parse_schema(text: &str) -> Value {
    serde_json::from_str(text).expect("protocol schemas are valid JSON")
}

/// The command's public description: class read, schemas from `protocol/`.
pub fn spec() -> CommandSpec {
    CommandSpec {
        id: CommandId::new(DIAGNOSTICS_LIST).expect("valid id"),
        title: "Error List: List Diagnostics".into(),
        input_schema: parse_schema(INPUT_SCHEMA),
        output_schema: parse_schema(OUTPUT_SCHEMA),
        permission: PermissionClass::Read,
        agent_visible: true,
    }
}

/// Register `diagnostics.list`, reading rows from `source`.
pub fn register(
    registry: &mut CommandRegistry,
    source: DiagnosticSource,
) -> Result<(), CommandError> {
    registry.register(spec(), move |input| {
        // MCP clients may send no arguments at all; treat null as `{}`.
        let input: Input = if input.is_null() {
            Input::default()
        } else {
            serde_json::from_value(input).map_err(|e| CommandError::InvalidInput(e.to_string()))?
        };
        let rows: Vec<Diagnostic> = source()
            .into_iter()
            .filter(|d| input.severity.is_none_or(|s| d.severity == s))
            .collect();
        serde_json::to_value(rows).map_err(|e| CommandError::Failed(e.to_string()))
    })
}

/// The fixed Error List used by brief 0005 (no host or compiler involved):
/// seven rows across three files. `OrderController.cs` has the most (three
/// errors); every other file has one error or none.
pub fn fixture() -> Vec<Diagnostic> {
    let d = |path: &str, line, column, severity, code: &str, message: &str| Diagnostic {
        path: path.into(),
        line,
        column,
        severity,
        code: code.into(),
        message: message.into(),
        project: None,
    };
    use Severity::*;
    const ORDERS: &str = "src/Contoso.Web/Controllers/OrderController.cs";
    const PRICING: &str = "src/Contoso.Core/Services/PricingService.cs";
    const CUSTOMER: &str = "src/Contoso.Core/Models/Customer.cs";
    vec![
        d(
            ORDERS,
            42,
            17,
            Error,
            "CS0103",
            "The name 'orderTotal' does not exist in the current context",
        ),
        d(ORDERS, 57, 34, Error, "CS1002", "; expected"),
        d(
            ORDERS,
            88,
            24,
            Error,
            "CS0029",
            "Cannot implicitly convert type 'string' to 'int'",
        ),
        d(
            PRICING,
            15,
            9,
            Error,
            "CS0246",
            "The type or namespace name 'DiscountPolicy' could not be found (are you missing a using directive or an assembly reference?)",
        ),
        d(
            PRICING,
            73,
            30,
            Warning,
            "CS0168",
            "The variable 'ex' is declared but never used",
        ),
        d(
            CUSTOMER,
            12,
            19,
            Warning,
            "CS8618",
            "Non-nullable property 'Email' must contain a non-null value when exiting constructor. Consider adding the 'required' modifier or declaring the property as nullable.",
        ),
        d(
            CUSTOMER,
            3,
            1,
            Message,
            "IDE0005",
            "Using directive is unnecessary.",
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn registry() -> CommandRegistry {
        let mut r = CommandRegistry::new();
        register(&mut r, Arc::new(fixture)).unwrap();
        r
    }

    #[test]
    fn spec_is_read_with_protocol_schemas() {
        let s = spec();
        assert_eq!(s.permission, PermissionClass::Read);
        assert_eq!(s.input_schema["type"], "object");
        assert_eq!(
            s.input_schema["properties"]["severity"]["enum"],
            json!(["error", "warning", "message"])
        );
        assert_eq!(s.output_schema["type"], "array");
        assert_eq!(
            s.output_schema["items"]["required"],
            json!(["path", "line", "column", "severity", "code", "message"])
        );
    }

    #[test]
    fn fixture_shape() {
        let f = fixture();
        assert!(f.len() >= 5);
        let mut files: Vec<_> = f.iter().map(|d| d.path.as_str()).collect();
        files.sort();
        files.dedup();
        assert!(files.len() >= 3);
        assert!(f.iter().all(|d| d.line >= 1 && d.column >= 1));
    }

    #[test]
    fn lists_all_and_filters() {
        let r = registry();
        let all = r.invoke(DIAGNOSTICS_LIST, json!({})).unwrap();
        assert_eq!(all, serde_json::to_value(fixture()).unwrap());
        assert_eq!(r.invoke(DIAGNOSTICS_LIST, Value::Null).unwrap(), all);

        let errors = r
            .invoke(DIAGNOSTICS_LIST, json!({"severity": "error"}))
            .unwrap();
        let errors: Vec<Diagnostic> = serde_json::from_value(errors).unwrap();
        assert_eq!(errors.len(), 4);
        assert!(errors.iter().all(|d| d.severity == Severity::Error));
        let messages = r
            .invoke(DIAGNOSTICS_LIST, json!({"severity": "message"}))
            .unwrap();
        assert_eq!(messages.as_array().unwrap().len(), 1);
    }

    #[test]
    fn rejects_bad_input() {
        let r = registry();
        for bad in [
            json!({"severity": "fatal"}),
            json!({"sev": "error"}),
            json!("error"),
        ] {
            assert!(
                matches!(
                    r.invoke(DIAGNOSTICS_LIST, bad.clone()),
                    Err(CommandError::InvalidInput(_))
                ),
                "{bad}"
            );
        }
        let audit = r.audit_log().entries();
        assert_eq!(audit.len(), 3);
        assert!(
            audit
                .iter()
                .all(|e| e.permission == Some(PermissionClass::Read) && !e.is_ok())
        );
    }
}
