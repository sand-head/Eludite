//! Server, transport and schema tests for `diagnostics.list` over MCP.

use std::io::Cursor;
use std::sync::{Arc, Mutex};

use niello_commands::diagnostics::{self, DIAGNOSTICS_LIST};
use niello_commands::{CommandId, CommandRegistry, CommandSpec, PermissionClass, builtins};
use serde_json::{Value, json};

use crate::transport::{listen_local, relay, serve_lines};
use crate::{McpServer, Message, Request, Response, ToolCallRecord, jsonrpc::ResponsePayload};

/// The subset of JSON Schema 2020-12 our schemas use: `type`, `enum`, `const`,
/// `properties`, `required`, `additionalProperties: false`, `items`,
/// `minimum`, `minLength`. Returns every violation found.
pub(crate) fn validate(schema: &Value, value: &Value) -> Vec<String> {
    let mut errors = Vec::new();
    check(schema, value, "$", &mut errors);
    errors
}

fn check(schema: &Value, v: &Value, at: &str, errors: &mut Vec<String>) {
    let Some(s) = schema.as_object() else { return };
    if let Some(t) = s.get("type").and_then(Value::as_str) {
        let ok = match t {
            "object" => v.is_object(),
            "array" => v.is_array(),
            "string" => v.is_string(),
            "integer" => v.is_i64() || v.is_u64(),
            "number" => v.is_number(),
            "boolean" => v.is_boolean(),
            "null" => v.is_null(),
            _ => true,
        };
        if !ok {
            errors.push(format!("{at}: expected {t}, got {v}"));
            return;
        }
    }
    if let Some(e) = s.get("enum").and_then(Value::as_array)
        && !e.contains(v)
    {
        errors.push(format!("{at}: {v} not in {e:?}"));
    }
    if let Some(c) = s.get("const")
        && c != v
    {
        errors.push(format!("{at}: {v} != const {c}"));
    }
    if let (Some(min), Some(n)) = (s.get("minimum").and_then(Value::as_f64), v.as_f64())
        && n < min
    {
        errors.push(format!("{at}: {n} < minimum {min}"));
    }
    if let (Some(min), Some(st)) = (s.get("minLength").and_then(Value::as_u64), v.as_str())
        && (st.chars().count() as u64) < min
    {
        errors.push(format!("{at}: shorter than {min}"));
    }
    if let Some(obj) = v.as_object() {
        let props = s.get("properties").and_then(Value::as_object);
        for r in s
            .get("required")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let r = r.as_str().unwrap_or_default();
            if !obj.contains_key(r) {
                errors.push(format!("{at}: missing required `{r}`"));
            }
        }
        for (k, val) in obj {
            match props.and_then(|p| p.get(k)) {
                Some(ps) => check(ps, val, &format!("{at}.{k}"), errors),
                None if s.get("additionalProperties") == Some(&Value::Bool(false)) => {
                    errors.push(format!("{at}: unexpected property `{k}`"));
                }
                None => {}
            }
        }
    }
    if let (Some(items), Some(arr)) = (s.get("items"), v.as_array()) {
        for (i, item) in arr.iter().enumerate() {
            check(items, item, &format!("{at}[{i}]"), errors);
        }
    }
}

fn registry() -> Arc<CommandRegistry> {
    let mut r = builtins::default_registry();
    diagnostics::register(&mut r, Arc::new(diagnostics::fixture)).unwrap();
    Arc::new(r)
}

fn diag_id() -> CommandId {
    CommandId::new(DIAGNOSTICS_LIST).unwrap()
}

fn server() -> McpServer {
    McpServer::new(registry(), [diag_id()])
}

fn call(server: &McpServer, method: &str, params: Value) -> Response {
    server
        .handle(Message::Request(Request::new(1, method, Some(params))))
        .expect("requests get a reply")
}

fn result(r: Response) -> Value {
    match r.payload {
        ResponsePayload::Result(v) => v,
        ResponsePayload::Error(e) => panic!("error {e:?}"),
    }
}

fn error_code(r: Response) -> i64 {
    match r.payload {
        ResponsePayload::Error(e) => e.code,
        ResponsePayload::Result(v) => panic!("expected error, got {v}"),
    }
}

#[test]
fn validator_rejects_what_it_should() {
    let out: Value = serde_json::from_str(diagnostics::OUTPUT_SCHEMA).unwrap();
    assert!(validate(&out, &serde_json::to_value(diagnostics::fixture()).unwrap()).is_empty());
    let bad = json!([
        {"path": "", "line": 0, "column": 1, "severity": "fatal", "code": "X", "message": "m", "extra": 1},
        {"path": "a.cs", "line": 1}
    ]);
    let errs = validate(&out, &bad);
    for needle in [
        "shorter",
        "minimum",
        "not in",
        "unexpected property",
        "missing required `column`",
    ] {
        assert!(
            errs.iter().any(|e| e.contains(needle)),
            "{needle} not in {errs:?}"
        );
    }
}

#[test]
fn initialize_negotiates_version() {
    let s = server();
    let r = result(call(
        &s,
        "initialize",
        json!({"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "t", "version": "0"}}),
    ));
    assert_eq!(r["protocolVersion"], "2025-06-18");
    assert_eq!(r["capabilities"]["tools"]["listChanged"], false);
    assert_eq!(r["serverInfo"]["name"], "niello");
    let r = result(call(
        &s,
        "initialize",
        json!({"protocolVersion": "1999-01-01"}),
    ));
    assert_eq!(r["protocolVersion"], crate::SUPPORTED_PROTOCOL_VERSIONS[0]);
    assert_eq!(result(call(&s, "ping", json!({}))), json!({}));
    assert_eq!(error_code(call(&s, "resources/list", json!({}))), -32601);
}

#[test]
fn lists_exactly_the_exposed_tool() {
    let r = result(call(&server(), "tools/list", json!({})));
    let tools = r["tools"].as_array().unwrap();
    assert_eq!(tools.len(), 1, "{r}");
    let t = &tools[0];
    assert_eq!(t["name"], "diagnostics-list");
    assert_eq!(t["title"], "Error List: List Diagnostics");
    assert!(t["description"].as_str().unwrap().contains("Error List"));
    assert_eq!(t["annotations"]["readOnlyHint"], true);
    assert!(t["inputSchema"].get("$schema").is_none());
    assert_eq!(
        t["inputSchema"]["properties"]["severity"]["enum"],
        json!(["error", "warning", "message"])
    );
    // MCP needs an object output schema: the array is wrapped as `result`.
    assert_eq!(t["outputSchema"]["type"], "object");
    assert_eq!(t["outputSchema"]["properties"]["result"]["type"], "array");
}

#[test]
fn call_returns_fixture_matching_schema() {
    let s = server();
    let out = result(call(
        &s,
        "tools/call",
        json!({"name": "diagnostics-list", "arguments": {}}),
    ));
    assert_eq!(out["isError"], false);
    let rows = &out["structuredContent"]["result"];
    assert_eq!(rows, &serde_json::to_value(diagnostics::fixture()).unwrap());

    // The command's own schema (protocol/) and the MCP-wrapped one both hold.
    let spec: &CommandSpec = s.registry().lookup(DIAGNOSTICS_LIST).unwrap();
    let errs = validate(&spec.output_schema, rows);
    assert!(errs.is_empty(), "{errs:?}");
    let tool = crate::tool_from_command(spec);
    assert!(validate(&tool.output_schema, &out["structuredContent"]).is_empty());
    // The text block carries the same JSON for clients without structured output.
    let text: Value = serde_json::from_str(out["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(&text, rows);

    // Arguments are validated against the input schema too.
    assert!(validate(&spec.input_schema, &json!({"severity": "error"})).is_empty());
    let errors = result(call(
        &s,
        "tools/call",
        json!({"name": "diagnostics-list", "arguments": {"severity": "error"}}),
    ));
    let errors = errors["structuredContent"]["result"]
        .as_array()
        .unwrap()
        .clone();
    assert_eq!(errors.len(), 4);
    assert!(errors.iter().all(|d| d["severity"] == "error"));
    // A missing `arguments` is `{}`.
    let all = result(call(&s, "tools/call", json!({"name": "diagnostics-list"})));
    assert_eq!(
        all["structuredContent"]["result"].as_array().unwrap().len(),
        7
    );
}

#[test]
fn bad_calls() {
    let s = server();
    // Invalid input is a tool error the model can read, not a protocol error.
    let r = result(call(
        &s,
        "tools/call",
        json!({"name": "diagnostics-list", "arguments": {"severity": "fatal"}}),
    ));
    assert_eq!(r["isError"], true);
    assert!(
        r["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("invalid input")
    );
    // Registered but not exposed, unknown, and malformed names are protocol errors.
    for name in [json!("niello-help-about"), json!("nope"), json!(3)] {
        assert_eq!(
            error_code(call(&s, "tools/call", json!({"name": name}))),
            -32602,
            "{name}"
        );
    }
    assert_eq!(error_code(s.handle_line("{not json").unwrap()), -32700);
    assert!(
        s.handle_line(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#)
            .is_none()
    );
}

#[test]
fn non_read_commands_need_the_gate() {
    let mut r = CommandRegistry::new();
    let id = CommandId::new("build.solution").unwrap();
    r.register(
        CommandSpec {
            id: id.clone(),
            title: "Build: Build Solution".into(),
            input_schema: json!({"type": "object", "properties": {}}),
            output_schema: json!({"type": "object", "properties": {"ok": {"type": "boolean"}}}),
            permission: PermissionClass::Execute,
        },
        |_| Ok(json!({"ok": true})),
    )
    .unwrap();
    let r = Arc::new(r);
    let denied = McpServer::new(r.clone(), [id.clone()]);
    let out = result(call(
        &denied,
        "tools/call",
        json!({"name": "build-solution", "arguments": {}}),
    ));
    assert_eq!(out["isError"], true);
    assert!(
        out["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("permission denied")
    );

    let asked = Arc::new(Mutex::new(Vec::new()));
    let asked2 = asked.clone();
    let allowed = McpServer::new(r, [id]).with_permission_gate(Arc::new(move |spec, _| {
        asked2.lock().unwrap().push(spec.id.to_string());
        true
    }));
    let out = result(call(
        &allowed,
        "tools/call",
        json!({"name": "build-solution", "arguments": {}}),
    ));
    assert_eq!(out["structuredContent"], json!({"ok": true}));
    assert_eq!(*asked.lock().unwrap(), ["build.solution"]);
}

#[test]
fn read_never_reaches_gate_and_is_audited() {
    let records: Arc<Mutex<Vec<ToolCallRecord>>> = Arc::default();
    let rec2 = records.clone();
    let s = server()
        .with_permission_gate(Arc::new(|_, _| panic!("read must not prompt")))
        .with_observer(Arc::new(move |r| rec2.lock().unwrap().push(r.clone())));
    result(call(
        &s,
        "tools/call",
        json!({"name": "diagnostics-list", "arguments": {}}),
    ));
    let records = records.lock().unwrap();
    assert_eq!(records.len(), 1);
    assert_eq!(
        records[0].command.as_ref().unwrap().as_str(),
        DIAGNOSTICS_LIST
    );
    assert_eq!(records[0].permission, Some(PermissionClass::Read));
    assert!(records[0].outcome.is_ok());
    let audit = s.registry().audit_log().entries();
    assert_eq!(audit.len(), 1);
    assert_eq!(audit[0].command, DIAGNOSTICS_LIST);
}

const SESSION: &str = concat!(
    r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"t","version":"0"}}}"#,
    "\n",
    r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
    "\n\n",
    r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#,
    "\n",
    r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"diagnostics-list","arguments":{"severity":"warning"}}}"#,
    "\n",
);

fn check_session_output(out: &[u8]) {
    let replies: Vec<Value> = String::from_utf8_lossy(out)
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert_eq!(replies.len(), 3, "{replies:?}");
    assert_eq!(
        replies
            .iter()
            .map(|r| r["id"].as_i64().unwrap())
            .collect::<Vec<_>>(),
        [1, 2, 3]
    );
    assert_eq!(replies[1]["result"]["tools"][0]["name"], "diagnostics-list");
    assert_eq!(
        replies[2]["result"]["structuredContent"]["result"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
}

#[test]
fn stdio_framing() {
    let mut out = Vec::new();
    serve_lines(&server(), Cursor::new(SESSION), &mut out).unwrap();
    check_session_output(&out);
}

#[test]
fn local_endpoint_through_relay() {
    let endpoint = listen_local(Arc::new(server())).unwrap();
    assert!(endpoint.addr.ip().is_loopback());
    assert_eq!(endpoint.token.len(), 32);
    let mut out = Vec::new();
    relay(
        endpoint.addr,
        &endpoint.token,
        Cursor::new(SESSION),
        &mut out,
    )
    .unwrap();
    check_session_output(&out);
}

#[test]
fn local_endpoint_rejects_wrong_token() {
    let endpoint = listen_local(Arc::new(server())).unwrap();
    let mut out = Vec::new();
    relay(
        endpoint.addr,
        "not-the-token",
        Cursor::new(SESSION),
        &mut out,
    )
    .unwrap();
    assert!(out.is_empty(), "{}", String::from_utf8_lossy(&out));
}
