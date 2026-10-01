//! Pure mapping between `claude`'s stream-json shapes and ACP types.
//!
//! The tool-call presentation (title, kind, locations, diff content) and the
//! permission options follow the mapping of the Apache-2.0
//! `@agentclientprotocol/claude-agent-acp` adapter (see NOTICE); the code is
//! written from scratch against the shapes recorded from `claude` itself.

use std::path::{Path, PathBuf};

use agent_client_protocol::schema::v1::{
    ContentBlock, Diff, McpServer, Meta, PermissionOption, PermissionOptionKind, ToolCallContent,
    ToolCallLocation, ToolCallStatus, ToolCallUpdateFields, ToolKind,
};
use serde_json::{Map, Value, json};

/// The presentation of one tool use.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolInfo {
    pub title: String,
    pub kind: ToolKind,
    pub locations: Vec<ToolCallLocation>,
    pub content: Vec<ToolCallContent>,
}

fn str_field<'a>(input: &'a Value, key: &str) -> Option<&'a str> {
    input
        .get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
}

/// `path` relative to `cwd` when it is inside it, for titles.
pub fn display_path(path: &str, cwd: &Path) -> String {
    Path::new(path)
        .strip_prefix(cwd)
        .ok()
        .filter(|p| !p.as_os_str().is_empty())
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_owned())
}

fn location(path: &str, line: Option<u32>) -> ToolCallLocation {
    ToolCallLocation::new(PathBuf::from(path)).line(line)
}

/// Title, kind, locations and content for a tool use. `input` may be `{}`
/// (the tool use has just started streaming); the title then falls back to the
/// tool's name.
pub fn tool_info(name: &str, input: &Value, cwd: &Path) -> ToolInfo {
    let mut info = ToolInfo {
        title: name.to_owned(),
        kind: ToolKind::Other,
        locations: Vec::new(),
        content: Vec::new(),
    };
    let path = str_field(input, "file_path").or_else(|| str_field(input, "notebook_path"));
    match name {
        "Read" => {
            info.kind = ToolKind::Read;
            if let Some(p) = path {
                info.title = format!("Read {}", display_path(p, cwd));
                let line = input
                    .get("offset")
                    .and_then(Value::as_u64)
                    .and_then(|n| u32::try_from(n).ok());
                info.locations.push(location(p, line));
            }
        }
        "Write" => {
            info.kind = ToolKind::Edit;
            if let Some(p) = path {
                info.title = format!("Write {}", display_path(p, cwd));
                info.locations.push(location(p, None));
                if let Some(text) = input.get("content").and_then(Value::as_str) {
                    info.content
                        .push(ToolCallContent::Diff(Diff::new(PathBuf::from(p), text)));
                }
            }
        }
        "Edit" | "MultiEdit" | "NotebookEdit" => {
            info.kind = ToolKind::Edit;
            if let Some(p) = path {
                info.title = format!("Edit {}", display_path(p, cwd));
                info.locations.push(location(p, None));
                if let (Some(old), Some(new)) = (
                    input.get("old_string").and_then(Value::as_str),
                    input.get("new_string").and_then(Value::as_str),
                ) {
                    info.content.push(ToolCallContent::Diff(
                        Diff::new(PathBuf::from(p), new).old_text(old.to_owned()),
                    ));
                }
            }
        }
        "Bash" => {
            info.kind = ToolKind::Execute;
            info.title = str_field(input, "command").unwrap_or("Terminal").to_owned();
            if let Some(d) = str_field(input, "description") {
                info.content.push(text_content(d));
            }
        }
        "Glob" => {
            info.kind = ToolKind::Search;
            if let Some(p) = str_field(input, "pattern") {
                info.title = format!("Find `{p}`");
            }
            if let Some(dir) = str_field(input, "path") {
                info.title
                    .push_str(&format!(" in {}", display_path(dir, cwd)));
                info.locations.push(location(dir, None));
            }
        }
        "Grep" => {
            info.kind = ToolKind::Search;
            if let Some(p) = str_field(input, "pattern") {
                info.title = format!("grep `{p}`");
            }
            if let Some(dir) = str_field(input, "path") {
                info.title
                    .push_str(&format!(" in {}", display_path(dir, cwd)));
                info.locations.push(location(dir, None));
            }
        }
        "WebFetch" => {
            info.kind = ToolKind::Fetch;
            if let Some(u) = str_field(input, "url") {
                info.title = format!("Fetch {u}");
            }
        }
        "WebSearch" => {
            info.kind = ToolKind::Fetch;
            if let Some(q) = str_field(input, "query") {
                info.title = format!("Search \"{q}\"");
            }
        }
        "Task" | "Agent" => {
            info.kind = ToolKind::Think;
            if let Some(d) = str_field(input, "description") {
                info.title = d.to_owned();
            }
        }
        "TodoWrite" => {
            info.kind = ToolKind::Think;
            info.title = "Update TODOs".into();
        }
        "ExitPlanMode" => {
            info.kind = ToolKind::SwitchMode;
            info.title = "Ready to code?".into();
        }
        _ => {}
    }
    info
}

/// The `_meta` Niello's panel (and other clients of the Node adapter) read the
/// agent's own tool name from: `_meta.claudeCode.toolName`.
pub fn tool_meta(name: &str, mcp_server: Option<&Value>) -> Meta {
    let mut cc = Map::new();
    cc.insert("toolName".into(), Value::String(name.to_owned()));
    if let Some(s) = mcp_server {
        cc.insert("mcpServer".into(), s.clone());
    }
    let mut meta = Meta::new();
    meta.insert("claudeCode".into(), Value::Object(cc));
    meta
}

pub fn text_content(text: &str) -> ToolCallContent {
    ToolCallContent::from(ContentBlock::from(text.to_owned()))
}

/// Text of a `tool_result` block's `content` (a string, or an array of
/// blocks). `tool_reference` blocks (from `ToolSearch`) render as
/// `Tool: <name>`.
pub fn tool_result_text(content: &Value) -> String {
    match content {
        Value::String(s) => s.clone(),
        Value::Array(items) => items
            .iter()
            .filter_map(|b| match b.get("type").and_then(Value::as_str) {
                Some("text") => b.get("text").and_then(Value::as_str).map(str::to_owned),
                Some("tool_reference") => b
                    .get("tool_name")
                    .and_then(Value::as_str)
                    .map(|n| format!("Tool: {n}")),
                Some(other) => Some(format!("[{other}]")),
                None => None,
            })
            .collect::<Vec<_>>()
            .join("\n"),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

/// The fields of the `tool_call_update` that ends a tool call. For a
/// successful edit the diff already shown stays (no content); otherwise the
/// result text replaces the content.
pub fn tool_result_fields(block: &Value, kind: Option<ToolKind>) -> ToolCallUpdateFields {
    let failed = block.get("is_error").and_then(Value::as_bool) == Some(true);
    let raw = block.get("content").cloned().unwrap_or(Value::Null);
    let text = tool_result_text(&raw);
    let mut fields = ToolCallUpdateFields::new()
        .status(if failed {
            ToolCallStatus::Failed
        } else {
            ToolCallStatus::Completed
        })
        .raw_output(raw);
    if failed || kind != Some(ToolKind::Edit) {
        fields = fields.content(if text.is_empty() {
            Vec::new()
        } else {
            vec![text_content(&text)]
        });
    }
    fields
}

/// Permission option ids the adapter offers.
pub mod option_ids {
    pub const ALLOW: &str = "allow";
    pub const ALLOW_ALWAYS: &str = "allow_always";
    pub const REJECT: &str = "reject";
    pub const REJECT_ALWAYS: &str = "reject_always";
}

/// The options for a `can_use_tool` request: allow once, allow always, reject
/// once, reject always.
pub fn permission_options(
    tool_name: &str,
    display: &str,
    suggestions: &Value,
) -> Vec<PermissionOption> {
    let accept_edits = suggestions.as_array().is_some_and(|s| {
        s.iter()
            .any(|x| x["type"] == "setMode" && x["mode"] == "acceptEdits")
    });
    let always = if accept_edits {
        "Yes, allow all edits during this session".to_owned()
    } else {
        format!("Yes, and don't ask again for {display}")
    };
    let _ = tool_name;
    vec![
        PermissionOption::new(option_ids::ALLOW, "Yes", PermissionOptionKind::AllowOnce),
        PermissionOption::new(
            option_ids::ALLOW_ALWAYS,
            always,
            PermissionOptionKind::AllowAlways,
        ),
        PermissionOption::new(option_ids::REJECT, "No", PermissionOptionKind::RejectOnce),
        PermissionOption::new(
            option_ids::REJECT_ALWAYS,
            format!("No, and don't ask again for {display}"),
            PermissionOptionKind::RejectAlways,
        ),
    ]
}

/// The message `claude` relays to the model when a tool is denied.
pub const DENY_MESSAGE: &str = "User refused permission to run tool";
pub const CANCELLED_MESSAGE: &str = "The prompt was cancelled";

/// The `control_response` payload for a `can_use_tool` request, given the
/// option the client selected (`None`: the client answered `cancelled`).
///
/// - allow once: `{behavior: allow, updatedInput}`;
/// - allow always: also `updatedPermissions`, the child's own
///   `permission_suggestions` (or a session rule for the tool);
/// - reject once: `{behavior: deny, message}`;
/// - reject always: also `updatedPermissions` with a session deny rule;
/// - cancelled: deny with `interrupt: true`.
pub fn permission_response(
    selected: Option<&str>,
    tool_name: &str,
    input: &Value,
    suggestions: &Value,
) -> Value {
    let rule = |behavior: &str| {
        json!([{
            "type": "addRules",
            "rules": [{"toolName": tool_name}],
            "behavior": behavior,
            "destination": "session",
        }])
    };
    match selected {
        Some(option_ids::ALLOW) => json!({"behavior": "allow", "updatedInput": input}),
        Some(option_ids::ALLOW_ALWAYS) => {
            let perms = match suggestions {
                Value::Array(a) if !a.is_empty() => suggestions.clone(),
                _ => rule("allow"),
            };
            json!({"behavior": "allow", "updatedInput": input, "updatedPermissions": perms})
        }
        Some(option_ids::REJECT_ALWAYS) => json!({
            "behavior": "deny",
            "message": DENY_MESSAGE,
            "updatedPermissions": rule("deny"),
        }),
        Some(_) => json!({"behavior": "deny", "message": DENY_MESSAGE}),
        None => json!({"behavior": "deny", "message": CANCELLED_MESSAGE, "interrupt": true}),
    }
}

/// The `--mcp-config` document for the client's `mcpServers`: stdio servers
/// as `{type: stdio, command, args, env}`, HTTP servers as
/// `{type: http, url, headers}`. SSE servers are not advertised and are
/// skipped.
pub fn mcp_config(servers: &[McpServer]) -> Value {
    let mut out = Map::new();
    for s in servers {
        match s {
            McpServer::Stdio(s) => {
                let env: Map<String, Value> = s
                    .env
                    .iter()
                    .map(|e| (e.name.clone(), Value::String(e.value.clone())))
                    .collect();
                out.insert(
                    s.name.clone(),
                    json!({
                        "type": "stdio",
                        "command": s.command.to_string_lossy(),
                        "args": s.args,
                        "env": env,
                    }),
                );
            }
            McpServer::Http(h) => {
                let headers: Map<String, Value> = h
                    .headers
                    .iter()
                    .map(|e| (e.name.clone(), Value::String(e.value.clone())))
                    .collect();
                out.insert(
                    h.name.clone(),
                    json!({"type": "http", "url": h.url, "headers": headers}),
                );
            }
            _ => {}
        }
    }
    json!({"mcpServers": out})
}

/// ACP prompt blocks to Anthropic content blocks. Text passes through; a
/// resource link becomes a text mention; embedded text resources become text
/// with their URI. Other kinds are not advertised and are dropped.
pub fn prompt_content(blocks: &[ContentBlock]) -> Vec<Value> {
    use agent_client_protocol::schema::v1::EmbeddedResourceResource;
    blocks
        .iter()
        .filter_map(|b| match b {
            ContentBlock::Text(t) => Some(json!({"type": "text", "text": t.text})),
            ContentBlock::ResourceLink(l) => {
                Some(json!({"type": "text", "text": format!("[@{}]({})", l.name, l.uri)}))
            }
            ContentBlock::Resource(r) => match &r.resource {
                EmbeddedResourceResource::TextResourceContents(t) => Some(json!({
                    "type": "text",
                    "text": format!("<context ref=\"{}\">\n{}\n</context>", t.uri, t.text),
                })),
                _ => None,
            },
            _ => None,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_client_protocol::schema::v1::{EnvVariable, McpServerHttp, McpServerStdio};

    #[test]
    fn write_maps_to_edit_with_diff_and_location() {
        let cwd = Path::new("/work");
        let i = tool_info(
            "Write",
            &json!({"file_path": "/work/notes.txt", "content": "hello\n"}),
            cwd,
        );
        assert_eq!(i.title, "Write notes.txt");
        assert_eq!(i.kind, ToolKind::Edit);
        assert_eq!(i.locations, vec![location("/work/notes.txt", None)]);
        assert_eq!(
            i.content,
            vec![ToolCallContent::Diff(Diff::new(
                "/work/notes.txt",
                "hello\n"
            ))]
        );
    }

    #[test]
    fn kinds_follow_the_node_adapter() {
        let cwd = Path::new("/w");
        let k = |n: &str, i: Value| tool_info(n, &i, cwd);
        assert_eq!(
            k("Read", json!({"file_path": "/w/a.rs", "offset": 10})).kind,
            ToolKind::Read
        );
        assert_eq!(
            k("Read", json!({"file_path": "/w/a.rs", "offset": 10})).locations[0].line,
            Some(10)
        );
        assert_eq!(
            k(
                "Edit",
                json!({"file_path": "/x/a.rs", "old_string": "a", "new_string": "b"})
            )
            .title,
            "Edit /x/a.rs"
        );
        assert_eq!(k("Bash", json!({"command": "ls"})).kind, ToolKind::Execute);
        assert_eq!(k("Bash", json!({"command": "ls"})).title, "ls");
        assert_eq!(k("Grep", json!({"pattern": "x"})).kind, ToolKind::Search);
        assert_eq!(
            k("Glob", json!({"pattern": "*.rs", "path": "/w/src"})).title,
            "Find `*.rs` in src"
        );
        assert_eq!(k("WebFetch", json!({"url": "u"})).kind, ToolKind::Fetch);
        assert_eq!(k("Task", json!({"description": "d"})).kind, ToolKind::Think);
        assert_eq!(k("ExitPlanMode", json!({})).kind, ToolKind::SwitchMode);
        let mcp = k(
            "mcp__niello__diagnostics-list",
            json!({"severity": "error"}),
        );
        assert_eq!(
            (mcp.title.as_str(), mcp.kind),
            ("mcp__niello__diagnostics-list", ToolKind::Other)
        );
        // Streaming start: no input yet, the title is the tool name.
        assert_eq!(k("Write", json!({})).title, "Write");
    }

    #[test]
    fn tool_results() {
        let ok = tool_result_fields(
            &json!({"type": "tool_result", "tool_use_id": "t", "content": [{"type": "tool_reference", "tool_name": "mcp__n__x"}]}),
            Some(ToolKind::Other),
        );
        assert_eq!(ok.status, Some(ToolCallStatus::Completed));
        assert_eq!(ok.content, Some(vec![text_content("Tool: mcp__n__x")]));
        let denied = tool_result_fields(
            &json!({"type": "tool_result", "tool_use_id": "t", "is_error": true, "content": "User refused permission to run tool"}),
            Some(ToolKind::Edit),
        );
        assert_eq!(denied.status, Some(ToolCallStatus::Failed));
        assert_eq!(
            denied.raw_output,
            Some(json!("User refused permission to run tool"))
        );
        let edited = tool_result_fields(
            &json!({"type": "tool_result", "tool_use_id": "t", "content": "updated"}),
            Some(ToolKind::Edit),
        );
        assert_eq!(edited.content, None, "a successful edit keeps its diff");
    }

    #[test]
    fn permission_outcomes_map_to_behaviours() {
        let input = json!({"file_path": "/w/n.txt", "content": "hi"});
        let sugg = json!([{"type": "setMode", "mode": "acceptEdits", "destination": "session"}]);
        assert_eq!(
            permission_response(Some("allow"), "Write", &input, &sugg),
            json!({"behavior": "allow", "updatedInput": input})
        );
        assert_eq!(
            permission_response(Some("allow_always"), "Write", &input, &sugg),
            json!({"behavior": "allow", "updatedInput": input, "updatedPermissions": sugg})
        );
        assert_eq!(
            permission_response(Some("allow_always"), "Write", &input, &Value::Null)["updatedPermissions"],
            json!([{"type": "addRules", "rules": [{"toolName": "Write"}], "behavior": "allow", "destination": "session"}])
        );
        assert_eq!(
            permission_response(Some("reject"), "Write", &input, &sugg),
            json!({"behavior": "deny", "message": DENY_MESSAGE})
        );
        let ra = permission_response(Some("reject_always"), "Write", &input, &sugg);
        assert_eq!(ra["behavior"], "deny");
        assert_eq!(ra["updatedPermissions"][0]["behavior"], "deny");
        let c = permission_response(None, "Write", &input, &sugg);
        assert_eq!(
            (c["behavior"].as_str(), c["interrupt"].as_bool()),
            (Some("deny"), Some(true))
        );

        let opts = permission_options("Write", "Write", &sugg);
        let kinds: Vec<_> = opts.iter().map(|o| o.kind).collect();
        assert_eq!(
            kinds,
            [
                PermissionOptionKind::AllowOnce,
                PermissionOptionKind::AllowAlways,
                PermissionOptionKind::RejectOnce,
                PermissionOptionKind::RejectAlways
            ]
        );
        assert_eq!(opts[1].name, "Yes, allow all edits during this session");
    }

    #[test]
    fn mcp_servers_become_a_strict_config() {
        let servers = vec![
            McpServer::Stdio(
                McpServerStdio::new("niello", "/opt/niello")
                    .args(vec!["--mcp-relay".into(), "127.0.0.1:1".into()])
                    .env(vec![EnvVariable::new("NIELLO_MCP_TOKEN", "t")]),
            ),
            McpServer::Http(McpServerHttp::new("web", "http://127.0.0.1:2/mcp")),
        ];
        assert_eq!(
            mcp_config(&servers),
            json!({"mcpServers": {
                "niello": {"type": "stdio", "command": "/opt/niello", "args": ["--mcp-relay", "127.0.0.1:1"], "env": {"NIELLO_MCP_TOKEN": "t"}},
                "web": {"type": "http", "url": "http://127.0.0.1:2/mcp", "headers": {}},
            }})
        );
        assert_eq!(mcp_config(&[]), json!({"mcpServers": {}}));
    }
}
