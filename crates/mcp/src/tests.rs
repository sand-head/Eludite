//! Server, transport and schema tests for `diagnostics.list` over MCP.

use std::io::Cursor;
use std::sync::{Arc, Mutex};

use eludite_commands::diagnostics::{self, DIAGNOSTICS_LIST};
use eludite_commands::{CommandId, CommandRegistry, CommandSpec, PermissionClass, builtins};
use serde_json::{Value, json};

use crate::transport::{listen_local, relay, serve_lines};
use crate::{
    GateDecision, McpServer, Message, Request, Response, ToolCallRecord, jsonrpc::ResponsePayload,
};

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
    McpServer::new(registry()).with_only([diag_id()])
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
    assert_eq!(r["capabilities"]["tools"]["listChanged"], true);
    assert_eq!(r["serverInfo"]["name"], "eludite");
    let r = result(call(
        &s,
        "initialize",
        json!({"protocolVersion": "1999-01-01"}),
    ));
    assert_eq!(r["protocolVersion"], crate::SUPPORTED_PROTOCOL_VERSIONS[0]);
    assert_eq!(result(call(&s, "ping", json!({}))), json!({}));
    assert_eq!(error_code(call(&s, "prompts/list", json!({}))), -32601);
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
    let spec: CommandSpec = s.registry().lookup(DIAGNOSTICS_LIST).unwrap();
    let errs = validate(&spec.output_schema, rows);
    assert!(errs.is_empty(), "{errs:?}");
    let tool = crate::tool_from_command(&spec);
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
    for name in [json!("eludite-help-about"), json!("nope"), json!(3)] {
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
    let r = CommandRegistry::new();
    let id = CommandId::new("build.solution").unwrap();
    r.register(
        CommandSpec {
            id: id.clone(),
            title: "Build: Build Solution".into(),
            input_schema: json!({"type": "object", "properties": {}}),
            output_schema: json!({"type": "object", "properties": {"ok": {"type": "boolean"}}}),
            permission: PermissionClass::Execute,
            agent_visible: true,
        },
        |_| Ok(json!({"ok": true})),
    )
    .unwrap();
    let r = Arc::new(r);
    let denied = McpServer::new(r.clone());
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
    let allowed = McpServer::new(r.clone())
        .with_agent("Fake")
        .with_permission_gate(Arc::new(move |spec, _, ctx| {
            asked2
                .lock()
                .unwrap()
                .push((spec.id.to_string(), ctx.clone()));
            GateDecision::Allow
        }));
    let out = result(call(
        &allowed,
        "tools/call",
        json!({"name": "build-solution", "arguments": {}}),
    ));
    assert_eq!(out["structuredContent"], json!({"ok": true}));
    let asked = asked.lock().unwrap();
    assert_eq!(asked.len(), 1);
    assert_eq!(asked[0].0, "build.solution");
    assert_eq!(asked[0].1.agent, "Fake");
    // Both calls are audited as the agent's, the denied one with the reason.
    let audit = r.audit_log().entries();
    assert_eq!(audit.len(), 2);
    assert!(audit.iter().all(|e| e.caller.is_agent()));
    assert!(
        matches!(&audit[0].outcome, eludite_commands::Outcome::Err(m) if m.contains("permission denied"))
    );
    assert!(audit[1].is_ok());
    assert_eq!(audit[1].caller.call(), Some(asked[0].1.call));
}

#[test]
fn read_never_reaches_gate_and_is_audited() {
    let records: Arc<Mutex<Vec<ToolCallRecord>>> = Arc::default();
    let rec2 = records.clone();
    let s = server()
        .with_permission_gate(Arc::new(|_, _, _| panic!("read must not prompt")))
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

// MCP conformance (brief 0016): the tool list is the bus's agent-visible commands, every descriptor follows
// `protocol/schemas/mcp-tool.json`, every command spec `command-spec.json`, and a command registered at runtime
// appears on the next list with a `notifications/tools/list_changed` first.

const MCP_TOOL_SCHEMA: &str = include_str!("../../../protocol/schemas/mcp-tool.json");
const COMMAND_SPEC_SCHEMA: &str = include_str!("../../../protocol/schemas/command-spec.json");

struct NoWorkspace;

impl eludite_commands::workspace::WorkspaceTarget for NoWorkspace {
    fn apply(
        &self,
        _: eludite_commands::workspace::WorkspaceRequest,
    ) -> Result<eludite_commands::workspace::WorkspaceOutput, eludite_commands::CommandError> {
        Err(eludite_commands::CommandError::Failed(
            "no workspace".into(),
        ))
    }
}

/// The shell's bus without the shell: built-ins, `diagnostics.list`, the workspace commands and the view commands.
fn full_registry() -> Arc<CommandRegistry> {
    let mut r = builtins::default_registry();
    diagnostics::register(&mut r, Arc::new(diagnostics::fixture)).unwrap();
    eludite_commands::workspace::register(&mut r, Arc::new(NoWorkspace));
    for id in eludite_commands::view::ALL {
        r.register(eludite_commands::view::spec(id), |_| Ok(json!({})))
            .unwrap();
    }
    Arc::new(r)
}

fn tool_names(list: &Value) -> Vec<String> {
    list["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap().to_owned())
        .collect()
}

#[test]
fn tools_list_reflects_the_bus_and_follows_the_schemas() {
    let r = full_registry();
    let s = McpServer::new(r.clone());
    let list = result(call(&s, "tools/list", json!({})));
    let names = tool_names(&list);
    let expected: Vec<String> = r
        .agent_visible()
        .iter()
        .map(|spec| crate::tool_name(&spec.id))
        .collect();
    assert_eq!(names, expected);
    for visible in [
        "diagnostics-list",
        "eludite-file-open",
        "eludite-workspace-apply_edit",
        "eludite-editor-rename",
        "eludite-editor-code_actions",
        "eludite-editor-go_to_definition",
        "eludite-editor-find_references",
    ] {
        assert!(names.iter().any(|n| n == visible), "{visible} in {names:?}");
    }
    for hidden in [
        "eludite-view-show",
        "eludite-view-toggle_tool_window",
        "eludite-navigation-back",
        "eludite-error_list-filter",
    ] {
        assert!(!names.iter().any(|n| n == hidden), "{hidden} hidden");
    }
    // Calling a UI-only command is an unknown tool.
    assert_eq!(
        error_code(call(&s, "tools/call", json!({"name": "eludite-view-show"}))),
        -32602
    );

    let tool_schema: Value = serde_json::from_str(MCP_TOOL_SCHEMA).unwrap();
    for t in list["tools"].as_array().unwrap() {
        let errs = validate(&tool_schema, t);
        assert!(errs.is_empty(), "{}: {errs:?}", t["name"]);
        // `pattern` is outside the validator's subset: check the name's form by hand.
        let name = t["name"].as_str().unwrap();
        assert!(
            name.chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-')
                && name.contains('-'),
            "{name}"
        );
        let id = crate::command_id_from_tool_name(name).unwrap();
        let spec = r.lookup(id.as_str()).unwrap();
        assert_eq!(t["_meta"]["eludite/permission"], spec.permission.as_str());
        let mut input = spec.input_schema.clone();
        input.as_object_mut().unwrap().remove("$schema");
        input.as_object_mut().unwrap().remove("$id");
        assert_eq!(t["inputSchema"], input, "{name}");
        assert_eq!(t["readOnlyHint"], Value::Null);
        assert_eq!(
            t["annotations"]["readOnlyHint"],
            spec.permission == PermissionClass::Read
        );
    }
    let spec_schema: Value = serde_json::from_str(COMMAND_SPEC_SCHEMA).unwrap();
    for spec in r.list() {
        let errs = validate(&spec_schema, &serde_json::to_value(&spec).unwrap());
        assert!(errs.is_empty(), "{}: {errs:?}", spec.id);
    }
}

fn read_json_line(reader: &mut impl std::io::BufRead) -> Value {
    let mut line = String::new();
    assert!(
        reader.read_line(&mut line).unwrap() > 0,
        "connection closed"
    );
    serde_json::from_str(&line).unwrap()
}

#[test]
fn a_command_registered_at_runtime_appears_on_the_next_list() {
    use std::io::{BufReader, Write as _};
    let r = full_registry();
    let before = r.agent_visible().len();
    let endpoint = listen_local(Arc::new(McpServer::new(r.clone()))).unwrap();
    let mut sock = std::net::TcpStream::connect(endpoint.addr).unwrap();
    sock.set_read_timeout(Some(std::time::Duration::from_secs(10)))
        .unwrap();
    let mut reader = BufReader::new(sock.try_clone().unwrap());
    writeln!(sock, "{}", endpoint.token).unwrap();
    writeln!(
        sock,
        r#"{{"jsonrpc":"2.0","id":1,"method":"initialize","params":{{"protocolVersion":"2025-06-18"}}}}"#
    )
    .unwrap();
    assert_eq!(read_json_line(&mut reader)["id"], 1);
    writeln!(sock, r#"{{"jsonrpc":"2.0","id":2,"method":"tools/list"}}"#).unwrap();
    let first = read_json_line(&mut reader);
    assert_eq!(tool_names(&first["result"]).len(), before);
    assert!(
        !tool_names(&first["result"])
            .iter()
            .any(|n| n == "eludite-test-late")
    );

    // A command added while the agent is connected: the server says the list changed, and the next list has it.
    r.register(
        CommandSpec {
            id: CommandId::new("eludite.test.late").unwrap(),
            title: "Test: Late".into(),
            input_schema: json!({"type": "object", "description": "Added at runtime.", "properties": {}}),
            output_schema: json!({"type": "object", "properties": {}}),
            permission: PermissionClass::Read,
            agent_visible: true,
        },
        |_| Ok(json!({})),
    )
    .unwrap();
    let note = read_json_line(&mut reader);
    assert_eq!(note["method"], "notifications/tools/list_changed");
    assert!(note.get("id").is_none());
    writeln!(sock, r#"{{"jsonrpc":"2.0","id":3,"method":"tools/list"}}"#).unwrap();
    let second = read_json_line(&mut reader);
    let names = tool_names(&second["result"]);
    assert_eq!(names.len(), before + 1);
    assert!(names.iter().any(|n| n == "eludite-test-late"));
    writeln!(
        sock,
        r#"{{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{{"name":"eludite-test-late"}}}}"#
    )
    .unwrap();
    assert_eq!(read_json_line(&mut reader)["result"]["isError"], false);
}

#[test]
fn a_waiting_call_does_not_hold_up_the_connection() {
    use std::io::{BufReader, Write as _};
    let r = full_registry();
    let (release_tx, release_rx) = std::sync::mpsc::channel::<()>();
    let release_rx = Arc::new(Mutex::new(release_rx));
    let threads: Arc<Mutex<Vec<String>>> = Arc::default();
    let threads2 = threads.clone();
    let server = McpServer::new(r)
        .with_agent("Fake")
        .with_permission_gate(Arc::new(move |_, _, _| {
            // The user takes a while to answer.
            let _ = release_rx
                .lock()
                .unwrap()
                .recv_timeout(std::time::Duration::from_secs(10));
            GateDecision::Deny("the user said no".into())
        }))
        .with_observer(Arc::new(move |r| {
            threads2.lock().unwrap().push(r.thread.clone())
        }));
    let endpoint = listen_local(Arc::new(server)).unwrap();
    let mut sock = std::net::TcpStream::connect(endpoint.addr).unwrap();
    sock.set_read_timeout(Some(std::time::Duration::from_secs(10)))
        .unwrap();
    let mut reader = BufReader::new(sock.try_clone().unwrap());
    writeln!(sock, "{}", endpoint.token).unwrap();
    // An execute command waits at the gate; a read call sent after it is answered first.
    writeln!(
        sock,
        r#"{{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{{"name":"eludite-solution-open","arguments":{{"path":"/x/A.sln"}}}}}}"#
    )
    .unwrap();
    writeln!(
        sock,
        r#"{{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{{"name":"diagnostics-list","arguments":{{}}}}}}"#
    )
    .unwrap();
    let first = read_json_line(&mut reader);
    assert_eq!(first["id"], 2);
    assert_eq!(first["result"]["isError"], false);
    release_tx.send(()).unwrap();
    let second = read_json_line(&mut reader);
    assert_eq!(second["id"], 1);
    assert_eq!(second["result"]["isError"], true);
    assert!(
        second["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("the user said no")
    );
    assert!(threads.lock().unwrap().iter().all(|t| t == "mcp-call"));
}

#[test]
fn the_invoker_runs_allowed_calls_with_the_agents_tool_call_id() {
    let seen: Arc<Mutex<Vec<crate::CallContext>>> = Arc::default();
    let seen2 = seen.clone();
    let s = McpServer::new(registry())
        .with_agent("Claude Code")
        .with_invoker(Arc::new(move |spec, args, ctx| {
            seen2.lock().unwrap().push(ctx.clone());
            eludite_commands::with_caller(ctx.caller(), || {
                Ok(json!({"wrapped": spec.id.to_string(), "args": args, "caller": eludite_commands::current_caller()}))
            })
        }));
    let out = result(call(
        &s,
        "tools/call",
        json!({"name": "eludite-help-about", "arguments": {}, "_meta": {"claudecode/toolUseId": "toolu_42"}}),
    ));
    assert_eq!(out["structuredContent"]["wrapped"], "eludite.help.about");
    assert_eq!(out["structuredContent"]["caller"]["kind"], "agent");
    let seen = seen.lock().unwrap();
    assert_eq!(seen[0].tool_call.as_deref(), Some("toolu_42"));
    assert_eq!(seen[0].agent, "Claude Code");
}

/// A 1x1 PNG, base64.
const PNG_1X1: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNkYPhfDwAChwGA60e6kgAAAABJRU5ErkJggg==";

#[test]
fn an_output_marked_as_image_is_sent_as_image_content_once() {
    let r = Arc::new(builtins::default_registry());
    let output_schema = json!({
        "type": "object",
        "required": ["image", "width"],
        "additionalProperties": false,
        "properties": {
            "image": {"type": "string", "contentEncoding": "base64", "contentMediaType": "image/*", "x-eludite-mcp-content": "image"},
            "thumb": {"type": "string", "contentEncoding": "base64", "contentMediaType": "image/jpeg", "x-eludite-mcp-content": "image"},
            "note": {"type": "string"},
            "width": {"type": "integer", "minimum": 0}
        }
    });
    r.register(
        CommandSpec {
            id: CommandId::new("eludite.test.picture").unwrap(),
            title: "Test: Picture".into(),
            input_schema: json!({"type": "object", "description": "A picture.", "properties": {}}),
            output_schema: output_schema.clone(),
            permission: PermissionClass::Read,
            agent_visible: true,
        },
        |_| Ok(json!({"image": PNG_1X1, "thumb": "/9j/AAAA", "note": PNG_1X1, "width": 1})),
    )
    .unwrap();
    let s = McpServer::new(r);
    let out = result(call(
        &s,
        "tools/call",
        json!({"name": "eludite-test-picture", "arguments": {}}),
    ));
    assert_eq!(out["isError"], false);
    let content = out["content"].as_array().unwrap();
    assert_eq!(
        content.len(),
        3,
        "text, then one image per marked property: {out}"
    );
    assert_eq!(content[0]["type"], "text");
    let text: Value = serde_json::from_str(content[0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(text["image"], crate::IMAGE_PLACEHOLDER);
    assert_eq!(text["thumb"], crate::IMAGE_PLACEHOLDER);
    // Only marked properties move; an unmarked string stays, base64 or not.
    assert_eq!(text["note"], PNG_1X1);
    assert_eq!(text["width"], 1);
    assert_eq!(out["structuredContent"], text);
    assert!(validate(&output_schema, &out["structuredContent"]).is_empty());
    // `image/*` is sniffed from the data; a declared type is kept.
    let images: Vec<(&str, &str)> = content[1..]
        .iter()
        .map(|c| {
            assert_eq!(c["type"], "image");
            (c["data"].as_str().unwrap(), c["mimeType"].as_str().unwrap())
        })
        .collect();
    assert!(images.contains(&(PNG_1X1, "image/png")), "{images:?}");
    assert!(images.contains(&("/9j/AAAA", "image/jpeg")), "{images:?}");
    // The marked image is sent once; the unmarked `note` (the same text) stays in the text part and the structured
    // content.
    assert_eq!(out.to_string().matches(PNG_1X1).count(), 3);
}

#[test]
fn image_types_are_sniffed() {
    use crate::sniff_image_type;
    assert_eq!(sniff_image_type(PNG_1X1), Some("image/png"));
    assert_eq!(sniff_image_type("/9j/4AAQSkZJRg=="), Some("image/jpeg"));
    assert_eq!(sniff_image_type("R0lGODlhAQABAA=="), Some("image/gif"));
    // "RIFF" + 4 bytes + "WEBP".
    assert_eq!(sniff_image_type("UklGRiQAAABXRUJQVlA4"), Some("image/webp"));
    assert_eq!(sniff_image_type("aGVsbG8="), None);
}

/// A command whose `to: "far"` calls are dangerous, `to: "no"` refused, and the rest execute (ADR-0009).
fn escalating_registry() -> Arc<CommandRegistry> {
    use eludite_commands::{Escalation, EscalationHook};
    let r = CommandRegistry::new();
    let hook: EscalationHook = Arc::new(|input: &Value, _view| match input["to"].as_str()? {
        "far" => Some(Escalation::raise(PermissionClass::Dangerous, "going far")),
        "no" => Some(Escalation::Refuse("the policy refuses it".into())),
        _ => None,
    });
    r.register_with_escalation(
        CommandSpec {
            id: CommandId::new("test.go").unwrap(),
            title: "Test: Go".into(),
            input_schema: json!({"type": "object", "description": "Go somewhere.", "x-eludite-escalates": "Dangerous when `to` is far.",
                "properties": {"to": {"type": "string"}}}),
            output_schema: json!({"type": "object", "properties": {"went": {"type": "string"}}}),
            permission: PermissionClass::Execute,
            agent_visible: true,
        },
        hook,
        |input| Ok(json!({"went": input["to"]})),
    )
    .unwrap();
    let clear: EscalationHook = Arc::new(|input: &Value, _view| {
        (input["clear"] == true).then(|| Escalation::raise(PermissionClass::Execute, "clearing"))
    });
    r.register_with_escalation(
        CommandSpec {
            id: CommandId::new("test.store").unwrap(),
            title: "Test: Store".into(),
            input_schema: json!({"type": "object", "x-eludite-escalates": "Execute when `clear` is true.", "properties": {"clear": {"type": "boolean"}}}),
            output_schema: json!({"type": "object", "properties": {}}),
            permission: PermissionClass::Read,
            agent_visible: true,
        },
        clear,
        |_| Ok(json!({})),
    )
    .unwrap();
    Arc::new(r)
}

#[test]
fn a_spec_with_a_hook_lists_when_it_escalates() {
    let s = McpServer::new(escalating_registry());
    let list = result(call(&s, "tools/list", json!({})));
    let go = list["tools"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["name"] == "test-go")
        .unwrap();
    assert_eq!(
        go["_meta"]["eludite/permission"], "execute",
        "the declared class"
    );
    assert_eq!(
        go["_meta"]["eludite/escalates"],
        "Dangerous when `to` is far."
    );
    assert!(
        go["inputSchema"].get("x-eludite-escalates").is_none(),
        "the key moves to _meta"
    );
    assert_eq!(go["annotations"]["destructiveHint"], false);
    let store = list["tools"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["name"] == "test-store")
        .unwrap();
    assert_eq!(store["_meta"]["eludite/permission"], "read");
    assert_eq!(
        store["annotations"]["readOnlyHint"], false,
        "a read command whose calls may escalate is not read-only"
    );
    // A spec without a hook has no `eludite/escalates`.
    let plain = crate::tool_from_command(
        &builtins::default_registry()
            .lookup(builtins::FILE_OPEN)
            .unwrap(),
    );
    assert!(
        serde_json::to_value(plain).unwrap()["_meta"]
            .get("eludite/escalates")
            .is_none()
    );
}

#[test]
fn the_gate_sees_the_effective_class() {
    let r = escalating_registry();
    type Seen = Vec<(String, PermissionClass, Option<String>)>;
    let seen: Arc<Mutex<Seen>> = Arc::default();
    let seen2 = seen.clone();
    let s = McpServer::new(r.clone())
        .with_agent("Fake")
        .with_permission_gate(Arc::new(move |spec, _, ctx| {
            seen2.lock().unwrap().push((
                spec.id.to_string(),
                ctx.class.class,
                ctx.class.reason.clone(),
            ));
            GateDecision::Allow
        }));
    let go = |to: &str| {
        result(call(
            &s,
            "tools/call",
            json!({"name": "test-go", "arguments": {"to": to}}),
        ))
    };
    assert_eq!(go("near")["isError"], false);
    assert_eq!(go("far")["structuredContent"]["went"], "far");
    // Refused calls never reach the gate.
    let refused = go("no");
    assert_eq!(refused["isError"], true);
    let text = refused["content"][0]["text"].as_str().unwrap().to_owned();
    assert!(text.contains("the policy refuses it"), "{text}");
    // A read call that escalates to execute goes to the gate; a plain one does not.
    for clear in [false, true] {
        let out = result(call(
            &s,
            "tools/call",
            json!({"name": "test-store", "arguments": {"clear": clear}}),
        ));
        assert_eq!(out["isError"], false);
    }
    let seen = seen.lock().unwrap().clone();
    assert_eq!(
        seen,
        [
            ("test.go".into(), PermissionClass::Execute, None),
            (
                "test.go".into(),
                PermissionClass::Dangerous,
                Some("going far".into())
            ),
            (
                "test.store".into(),
                PermissionClass::Execute,
                Some("clearing".into())
            ),
        ]
    );
    // The audit entries carry the effective class and the reason, the refusal included.
    let log = r.audit_log().entries();
    let classes: Vec<_> = log
        .iter()
        .map(|e| (e.permission.unwrap(), e.escalation.clone(), e.is_ok()))
        .collect();
    assert_eq!(
        classes,
        [
            (PermissionClass::Execute, None, true),
            (PermissionClass::Dangerous, Some("going far".into()), true),
            (
                PermissionClass::Execute,
                Some("the policy refuses it".into()),
                false
            ),
            (PermissionClass::Read, None, true),
            (PermissionClass::Execute, Some("clearing".into()), true),
        ]
    );
    // The denial names the effective class and the reason.
    let denying = McpServer::new(r.clone());
    let out = result(call(
        &denying,
        "tools/call",
        json!({"name": "test-go", "arguments": {"to": "far"}}),
    ));
    let text = out["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("is class dangerous (going far)"), "{text}");
}

// ----- Brief 0027: the guides as MCP resources. -----

const RESOURCE_SCHEMA: &str = include_str!("../../../protocol/schemas/mcp-resource.json");

#[test]
fn the_debugging_guide_is_a_resource() {
    let s = server();
    let init = result(call(
        &s,
        "initialize",
        json!({"protocolVersion": "2025-06-18", "capabilities": {}}),
    ));
    assert_eq!(init["capabilities"]["resources"], json!({}));
    assert!(
        init["instructions"]
            .as_str()
            .unwrap()
            .contains("eludite://guides/debugging")
    );
    let schema: Value = serde_json::from_str(RESOURCE_SCHEMA).unwrap();
    let list = result(call(&s, "resources/list", json!({})));
    let errors = validate(&schema["$defs"]["list_result"], &list);
    assert!(errors.is_empty(), "{errors:?}");
    let resources = list["resources"].as_array().unwrap();
    assert_eq!(
        resources.len(),
        3,
        "the debugging, git and terminal guides (no git status without its command)"
    );
    assert_eq!(resources[2]["uri"], "eludite://guides/terminal");
    for r in resources {
        let errors = validate(&schema, r);
        assert!(errors.is_empty(), "{errors:?}");
    }
    assert_eq!(resources[1]["uri"], "eludite://guides/git");
    assert_eq!(resources[0]["uri"], "eludite://guides/debugging");
    assert_eq!(resources[0]["mimeType"], "text/markdown");
    assert!(list.get("nextCursor").is_none());
    let read = result(call(
        &s,
        "resources/read",
        json!({"uri": "eludite://guides/debugging"}),
    ));
    let errors = validate(&schema["$defs"]["read_result"], &read);
    assert!(errors.is_empty(), "{errors:?}");
    let text = read["contents"][0]["text"].as_str().unwrap();
    assert_eq!(
        text,
        include_str!("../../../docs/agents/debugging.md"),
        "the file compiled in"
    );
    assert_eq!(resources[0]["size"], text.len());
    assert_eq!(
        error_code(call(
            &s,
            "resources/read",
            json!({"uri": "eludite://guides/nope"})
        )),
        crate::resources::RESOURCE_NOT_FOUND
    );
    assert_eq!(
        error_code(call(&s, "resources/read", json!({}))),
        crate::ErrorObject::INVALID_PARAMS
    );
    let templates = result(call(&s, "resources/templates/list", json!({})));
    let errors = validate(&schema["$defs"]["templates_list_result"], &templates);
    assert!(errors.is_empty(), "{errors:?}");
    assert_eq!(templates["resourceTemplates"], json!([]));
}

/// The guide stays short and names only commands that exist (brief 0027 Contract).
#[test]
fn the_debugging_guide_is_short_and_names_real_commands() {
    let text = crate::resources::DEBUGGING.text;
    let words = text.split_whitespace().count();
    assert!(words < 2_000, "{words} words");
    // Every `eludite.debug.<name>` it names is a debug command; the bare `<name>`s after a full id too.
    let ids: Vec<&str> = eludite_commands::debug::ALL.to_vec();
    let mut named = 0;
    for (i, _) in text.match_indices("eludite.debug.") {
        let rest = &text[i..];
        let end = rest
            .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == '.'))
            .unwrap_or(rest.len());
        let id = rest[..end].trim_end_matches('.');
        // `eludite.debug.*` and `eludite.debug.<name>` name the family.
        if id == "eludite.debug" {
            continue;
        }
        assert!(
            ids.contains(&id),
            "the guide names `{id}`, which is not a command"
        );
        named += 1;
    }
    assert!(named > 15, "{named}");
    for bare in [
        "snapshot",
        "stack",
        "variables",
        "output",
        "exception_info",
        "wait",
        "run_until",
        "trace",
        "continue",
        "step_over",
        "step_into",
        "step_out",
        "run_to_cursor",
        "pause",
        "set_variable",
        "set_next_statement",
        "toggle_breakpoint",
        "exception_settings",
        "start",
        "attach",
        "processes",
        "restart",
        "stop",
        "allow_agents",
        "evaluate",
        "select_frame",
        "state",
    ] {
        assert!(
            ids.contains(&format!("eludite.debug.{bare}").as_str()),
            "{bare}"
        );
    }
    // In the order of proposal 0001 section 9.
    let at = |s: &str| text.find(s).unwrap_or_else(|| panic!("{s}"));
    assert!(at("## 1. Read `snapshot`") < at("## 2. Prefer `run_until` and `trace`"));
    assert!(at("## 2.") < at("## 3. Pass `stop`"));
    assert!(at("## 3.") < at("## 4. Read `output` by cursor"));
    assert!(at("## 4.") < at("## 5. When a call returns `interrupted_by: \"user\"`"));
    assert!(at("## 5.") < at("## 6. What the policy may refuse"));
    assert!(text.contains(eludite_commands::debug::AGENTS_NOT_ALLOWED));
}

// ----- Brief 0040: the git guide and the live status resource. -----

struct FakeGit;

impl eludite_commands::git::GitCommands for FakeGit {
    fn apply(
        &self,
        request: eludite_commands::git::GitRequest,
    ) -> Result<eludite_commands::git::GitOutput, eludite_commands::CommandError> {
        use eludite_commands::git::{GitOutput, GitRequest, StatusOutput};
        match request {
            GitRequest::Status { .. } => Ok(GitOutput::Status(Box::new(StatusOutput {
                state: "ready".into(),
                repository: Some("/r".into()),
                generation: 7,
                branch: Some("main".into()),
                untracked: vec!["new.cs".into()],
                ..Default::default()
            }))),
            other => Err(eludite_commands::CommandError::Failed(format!("{other:?}"))),
        }
    }
}

#[test]
fn the_git_status_is_a_live_resource_read_as_the_agent() {
    let r = eludite_commands::CommandRegistry::new();
    eludite_commands::git::register(&r, std::sync::Arc::new(FakeGit));
    let s = McpServer::new(std::sync::Arc::new(r)).with_agent("claude");
    let schema: Value = serde_json::from_str(RESOURCE_SCHEMA).unwrap();
    let init = result(call(
        &s,
        "initialize",
        json!({"protocolVersion": "2025-06-18"}),
    ));
    assert!(
        init["instructions"]
            .as_str()
            .unwrap()
            .contains("eludite://guides/git")
    );
    let list = result(call(&s, "resources/list", json!({})));
    let resources = list["resources"].as_array().unwrap();
    assert_eq!(resources.len(), 4);
    assert_eq!(resources[3]["uri"], crate::resources::GIT_STATUS_URI);
    assert_eq!(resources[3]["mimeType"], "application/json");
    for r in resources {
        let errors = validate(&schema, r);
        assert!(errors.is_empty(), "{errors:?}");
    }
    let read = result(call(
        &s,
        "resources/read",
        json!({"uri": "eludite://git/status"}),
    ));
    let errors = validate(&schema["$defs"]["read_result"], &read);
    assert!(errors.is_empty(), "{errors:?}");
    let status: Value =
        serde_json::from_str(read["contents"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(status["generation"], 7);
    assert_eq!(status["untracked"], json!(["new.cs"]));
    // Read through the bus as the agent, so the audit log has it.
    let last = s.registry().audit_log().entries().pop().unwrap();
    assert_eq!(last.command, "eludite.git.status");
    assert!(last.caller.is_agent());
    // The guide.
    let guide = result(call(
        &s,
        "resources/read",
        json!({"uri": "eludite://guides/git"}),
    ));
    assert_eq!(
        guide["contents"][0]["text"].as_str().unwrap(),
        include_str!("../../../docs/agents/git.md")
    );
}

/// The git guide stays under 800 words (brief 0040) and names only commands that exist.
#[test]
fn the_git_guide_is_short_and_names_real_commands() {
    let text = crate::resources::GIT.text;
    let words = text.split_whitespace().count();
    assert!(words < 800, "{words} words");
    let mut named = 0;
    for (i, _) in text.match_indices("eludite.git.") {
        let rest = &text[i..];
        let end = rest
            .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == '.'))
            .unwrap_or(rest.len());
        let id = rest[..end].trim_end_matches('.');
        if id == "eludite.git" {
            continue;
        }
        assert!(
            eludite_commands::git::ALL.contains(&id),
            "the guide names `{id}`, which is not a command"
        );
        named += 1;
    }
    assert!(named > 12, "{named}");
    assert!(
        text.contains("git.push") && text.contains("git.history") && text.contains("git.commit")
    );
}

/// The terminal guide stays under 600 words (brief 0041), is served as `eludite://guides/terminal`, and names only
/// commands that exist.
#[test]
fn the_terminal_guide_is_short_served_and_names_real_commands() {
    let text = crate::resources::TERMINAL.text;
    let words = text.split_whitespace().count();
    assert!(words < 600, "{words} words");
    let mut named = 0;
    for (i, _) in text.match_indices("eludite.terminal.") {
        let rest = &text[i..];
        let end = rest
            .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == '.'))
            .unwrap_or(rest.len());
        let id = rest[..end].trim_end_matches('.');
        if id == "eludite.terminal" {
            continue;
        }
        assert!(
            eludite_commands::terminal::ALL.contains(&id),
            "the guide names `{id}`, which is not a command"
        );
        named += 1;
    }
    assert!(named >= 8, "{named}");
    assert!(text.contains("terminal.run") && text.contains("interrupted_by"));
    let s = McpServer::new(std::sync::Arc::new(eludite_commands::CommandRegistry::new()));
    let init = result(call(
        &s,
        "initialize",
        json!({"protocolVersion": "2025-06-18"}),
    ));
    assert!(
        init["instructions"]
            .as_str()
            .unwrap()
            .contains("eludite://guides/terminal")
    );
    let read = result(call(
        &s,
        "resources/read",
        json!({"uri": "eludite://guides/terminal"}),
    ));
    assert_eq!(read["contents"][0]["text"].as_str().unwrap(), text);
    assert_eq!(read["contents"][0]["mimeType"], "text/markdown");
}
