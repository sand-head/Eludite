//! Several language servers for one document (brief 0050): which of them a request goes to, and how their answers
//! merge into the one answer the editor features expect. TypeScript and ESLint share `*.ts`; the shell's composite
//! session and the tests below send each request to every server that offers it and merge with [`merge`].
//!
//! The rules (host-rpc.md, "Fan-out and merge"):
//!
//! - **Completion**: the lists concatenated (each list's `itemDefaults` written into its items first, since the merged
//!   list cannot carry two sets), `isIncomplete` if any is; every item names its server in `labelDetails.description`
//!   when that is empty, and carries it in `data` ([`ORIGIN`]) so `completionItem/resolve` goes back to it.
//! - **Code actions**: concatenated in server order, each carrying its server in `data` for `codeAction/resolve`.
//! - **Hover, signature help**: the first non-empty answer in server order.
//! - **Definition, references**: concatenated, duplicate locations dropped.
//! - **Formatting, rename, prepare rename** (and anything else): the first server, in order, that answers.
//! - **Resolve requests** go to the item's server only ([`route`]); `workspace/executeCommand` to the server whose
//!   `executeCommandProvider.commands` lists the command ([`command_owner`]).
//!
//! A server that does not offer a method ([`offers`]) is not asked. One that fails is left out of the merge; the
//! request fails only when every server that was asked failed.

use std::sync::Arc;

use serde_json::{Map, Value, json};

/// The member of `data` that names the server a merged completion item or code action came from.
pub const ORIGIN: &str = "eludite.server";
/// The member of `data` that holds the server's own `data`.
const INNER: &str = "data";

/// How one server answered a fanned-out request.
#[derive(Debug, Clone, PartialEq)]
pub enum Outcome {
    Answered(Value),
    /// It does not offer the method (it was not asked).
    NotOffered,
    Failed(String),
}

/// One server's part in a fanned-out request: its index in the document's server list, its name and its outcome.
#[derive(Debug, Clone, PartialEq)]
pub struct Answer {
    pub server: usize,
    pub name: String,
    pub outcome: Outcome,
}

/// How answers to a method merge.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rule {
    Completion,
    CodeActions,
    FirstNonEmpty,
    Locations,
    First,
}

/// The rule for `method`.
pub fn rule(method: &str) -> Rule {
    match method {
        "textDocument/completion" => Rule::Completion,
        "textDocument/codeAction" => Rule::CodeActions,
        "textDocument/hover" | "textDocument/signatureHelp" => Rule::FirstNonEmpty,
        "textDocument/definition"
        | "textDocument/references"
        | "textDocument/implementation"
        | "textDocument/typeDefinition" => Rule::Locations,
        _ => Rule::First,
    }
}

/// The server capability that offers `method`, when the method has one.
fn capability(method: &str) -> Option<&'static str> {
    Some(match method {
        "textDocument/completion" | "completionItem/resolve" => "completionProvider",
        "textDocument/hover" => "hoverProvider",
        "textDocument/signatureHelp" => "signatureHelpProvider",
        "textDocument/definition" => "definitionProvider",
        "textDocument/references" => "referencesProvider",
        "textDocument/implementation" => "implementationProvider",
        "textDocument/typeDefinition" => "typeDefinitionProvider",
        "textDocument/prepareRename" | "textDocument/rename" => "renameProvider",
        "textDocument/codeAction" | "codeAction/resolve" => "codeActionProvider",
        "textDocument/formatting" => "documentFormattingProvider",
        "textDocument/rangeFormatting" => "documentRangeFormattingProvider",
        "textDocument/documentSymbol" => "documentSymbolProvider",
        "textDocument/documentHighlight" => "documentHighlightProvider",
        "textDocument/semanticTokens/full" => "semanticTokensProvider",
        "textDocument/inlayHint" => "inlayHintProvider",
        "workspace/executeCommand" => "executeCommandProvider",
        _ => return None,
    })
}

/// Whether a server with `capabilities` offers `method` (a method with no capability is offered by all).
pub fn offers(method: &str, capabilities: &Value) -> bool {
    let Some(key) = capability(method) else {
        return true;
    };
    let cap = &capabilities[key];
    let offered = !cap.is_null() && cap != &Value::Bool(false);
    match method {
        "completionItem/resolve" | "codeAction/resolve" => {
            offered && cap["resolveProvider"].as_bool() == Some(true)
        }
        "textDocument/prepareRename" => offered && cap["prepareProvider"].as_bool() == Some(true),
        _ => offered,
    }
}

/// The server whose `executeCommandProvider.commands` lists `command`, among the servers' capabilities in order.
pub fn command_owner<'a>(
    command: &str,
    capabilities: impl IntoIterator<Item = Option<&'a Value>>,
) -> Option<usize> {
    capabilities.into_iter().position(|caps| {
        caps.and_then(|c| c["executeCommandProvider"]["commands"].as_array())
            .is_some_and(|cmds| cmds.iter().any(|c| c.as_str() == Some(command)))
    })
}

/// For a resolve request (`completionItem/resolve`, `codeAction/resolve`): the server the item came from and the
/// item as that server gave it (its own `data` restored). `None` for other methods, or an item without an origin.
pub fn route(method: &str, params: &Value) -> Option<(usize, Value)> {
    if method != "completionItem/resolve" && method != "codeAction/resolve" {
        return None;
    }
    let data = params.get("data")?;
    let server = data.get(ORIGIN)?.as_u64()? as usize;
    let mut item = params.clone();
    match data.get(INNER) {
        Some(inner) => item["data"] = inner.clone(),
        None => {
            if let Some(o) = item.as_object_mut() {
                o.remove("data");
            }
        }
    }
    Some((server, item))
}

/// Mark `item` (a completion item or code action) as server `server`'s: its `data` moves under [`ORIGIN`]'s sibling.
pub fn tag(item: &mut Value, server: usize) {
    let Some(o) = item.as_object_mut() else {
        return;
    };
    let mut data = Map::new();
    data.insert(ORIGIN.into(), json!(server));
    if let Some(inner) = o.remove("data") {
        data.insert(INNER.into(), inner);
    }
    o.insert("data".into(), Value::Object(data));
}

fn is_empty(v: &Value) -> bool {
    match v {
        Value::Null => true,
        Value::Array(a) => a.is_empty(),
        Value::Object(o) => {
            // A hover with empty contents, signature help with no signatures.
            o.get("contents").is_some_and(|c| {
                c.is_null()
                    || c.as_str() == Some("")
                    || c["value"].as_str() == Some("")
                    || c.as_array().is_some_and(Vec::is_empty)
            }) || o
                .get("signatures")
                .is_some_and(|s| s.as_array().is_some_and(Vec::is_empty))
        }
        _ => false,
    }
}

/// A completion list's items with its `itemDefaults` written into them.
fn completion_items(answer: &Value) -> (Vec<Value>, bool) {
    let (items, defaults, incomplete) = match answer {
        Value::Array(items) => (items.clone(), None, false),
        Value::Object(list) => (
            list.get("items")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default(),
            list.get("itemDefaults").cloned(),
            list.get("isIncomplete").and_then(Value::as_bool) == Some(true),
        ),
        _ => (Vec::new(), None, false),
    };
    let Some(defaults) = defaults else {
        return (items, incomplete);
    };
    let items = items
        .into_iter()
        .map(|mut item| {
            for key in ["commitCharacters", "insertTextFormat", "insertTextMode", "data"] {
                if item.get(key).is_none()
                    && let Some(v) = defaults.get(key)
                {
                    item[key] = v.clone();
                }
            }
            if item.get("textEdit").is_none()
                && let Some(range) = defaults.get("editRange")
            {
                let new_text = item
                    .get("textEditText")
                    .or_else(|| item.get("insertText"))
                    .or_else(|| item.get("label"))
                    .cloned()
                    .unwrap_or(Value::Null);
                item["textEdit"] = if range.get("insert").is_some() {
                    json!({"newText": new_text, "insert": range["insert"], "replace": range["replace"]})
                } else {
                    json!({"newText": new_text, "range": range})
                };
            }
            item
        })
        .collect();
    (items, incomplete)
}

/// Merge the answers to `method` (in server order) by its [`rule`]. Fails only when no server answered.
pub fn merge(method: &str, answers: Vec<Answer>) -> Result<Value, String> {
    let answered: Vec<(usize, String, Value)> = answers
        .iter()
        .filter_map(|a| match &a.outcome {
            Outcome::Answered(v) => Some((a.server, a.name.clone(), v.clone())),
            _ => None,
        })
        .collect();
    if answered.is_empty() {
        let failures: Vec<String> = answers
            .iter()
            .filter_map(|a| match &a.outcome {
                Outcome::Failed(e) => Some(format!("{}: {e}", a.name)),
                _ => None,
            })
            .collect();
        return if failures.is_empty() {
            Ok(Value::Null)
        } else {
            Err(failures.join("; "))
        };
    }
    Ok(match rule(method) {
        Rule::Completion => {
            let mut all = Vec::new();
            let mut incomplete = false;
            for (server, name, answer) in &answered {
                let (items, inc) = completion_items(answer);
                incomplete |= inc;
                for mut item in items {
                    if item["labelDetails"]["description"]
                        .as_str()
                        .is_none_or(str::is_empty)
                        && item.is_object()
                    {
                        if !item["labelDetails"].is_object() {
                            item["labelDetails"] = json!({});
                        }
                        item["labelDetails"]["description"] = json!(name);
                    }
                    tag(&mut item, *server);
                    all.push(item);
                }
            }
            json!({"isIncomplete": incomplete, "items": all})
        }
        Rule::CodeActions => {
            let mut all = Vec::new();
            for (server, _, answer) in &answered {
                for action in answer.as_array().into_iter().flatten() {
                    let mut action = action.clone();
                    // A bare `Command` is not a code action and has no `data`: it stays as it is.
                    if action.get("command").is_none_or(|c| !c.is_string()) {
                        tag(&mut action, *server);
                    }
                    all.push(action);
                }
            }
            Value::Array(all)
        }
        Rule::FirstNonEmpty => answered
            .iter()
            .map(|(_, _, v)| v)
            .find(|v| !is_empty(v))
            .cloned()
            .unwrap_or(Value::Null),
        Rule::Locations => {
            let mut all: Vec<Value> = Vec::new();
            for (_, _, answer) in &answered {
                let list = match answer {
                    Value::Array(a) => a.clone(),
                    Value::Null => Vec::new(),
                    one => vec![one.clone()],
                };
                for loc in list {
                    let key = (
                        loc.get("uri").or_else(|| loc.get("targetUri")).cloned(),
                        loc.get("range")
                            .or_else(|| loc.get("targetSelectionRange"))
                            .cloned(),
                    );
                    if !all.iter().any(|l| {
                        (
                            l.get("uri").or_else(|| l.get("targetUri")).cloned(),
                            l.get("range")
                                .or_else(|| l.get("targetSelectionRange"))
                                .cloned(),
                        ) == key
                    }) {
                        all.push(loc);
                    }
                }
            }
            Value::Array(all)
        }
        Rule::First => answered[0].2.clone(),
    })
}

/// The answer to a resolve request, as the merged list holds it: the server's `data` back under [`ORIGIN`].
pub fn retag(method: &str, server: usize, mut result: Value) -> Value {
    if (method == "completionItem/resolve" || method == "codeAction/resolve") && result.is_object()
    {
        tag(&mut result, server);
    }
    result
}

/// A request sent to one server: wait for its answer, or cancel it.
pub struct Sent {
    pub wait: Box<dyn FnOnce() -> Result<Value, String> + Send>,
    pub cancel: Box<dyn Fn() + Send + Sync>,
}

/// One of a document's servers, as [`dispatch`] sees it: the shell's session, or a [`crate::ServerClient`] in
/// tests.
pub trait Member: Send + Sync {
    /// The status bar's name for it (`TypeScript`).
    fn name(&self) -> String;
    /// Its `ServerCapabilities`, once it has started (`None`: not yet, so it is asked anyway).
    fn capabilities(&self) -> Option<Value>;
    /// Send `method` now (after the document notifications already sent to it).
    fn send(&self, method: &str, params: Value) -> Sent;
}

impl Member for crate::ServerClient {
    fn name(&self) -> String {
        self.server_info().map(|(n, _)| n).unwrap_or_default()
    }

    fn capabilities(&self) -> Option<Value> {
        crate::ServerClient::capabilities(self)
    }

    fn send(&self, method: &str, params: Value) -> Sent {
        match self.request_untyped(method, params) {
            Ok(pending) => {
                let id = pending.id();
                let conn = self.connection().clone();
                Sent {
                    wait: Box::new(move || pending.wait().map_err(|e| e.to_string())),
                    cancel: Box::new(move || conn.cancel_id(id.clone())),
                }
            }
            Err(e) => {
                let e = e.to_string();
                Sent {
                    wait: Box::new(move || Err(e)),
                    cancel: Box::new(|| {}),
                }
            }
        }
    }
}

/// Send `method` to the document's `members` (in order: the primary server first) by the rules above and merge the
/// answers. Blocks until they answer: run it off the UI thread. `sent` receives each request's cancel, so a caller
/// can cancel the fan-out while it waits.
pub fn dispatch(
    members: &[Arc<dyn Member>],
    method: &str,
    params: Value,
    sent: &dyn Fn(Box<dyn Fn() + Send + Sync>),
) -> Result<Value, String> {
    let send = |m: &Arc<dyn Member>, params: Value| {
        let s = m.send(method, params);
        sent(s.cancel);
        s.wait
    };
    if let Some((i, item)) = route(method, &params) {
        let Some(m) = members.get(i) else {
            return Err(format!("{method}: the item's server is gone"));
        };
        if m.capabilities().is_some_and(|c| !offers(method, &c)) {
            // Nothing to resolve: the item as it was.
            return Ok(retag(method, i, item));
        }
        return send(m, item)().map(|r| retag(method, i, r));
    }
    if method == "workspace/executeCommand" {
        let command = params["command"].as_str().unwrap_or_default().to_owned();
        let caps: Vec<Option<Value>> = members.iter().map(|m| m.capabilities()).collect();
        let owner = command_owner(&command, caps.iter().map(Option::as_ref))
            .ok_or_else(|| format!("no server runs the command {command}"))?;
        return send(&members[owner], params)();
    }
    let offered = |m: &Arc<dyn Member>| m.capabilities().is_none_or(|c| offers(method, &c));
    if rule(method) == Rule::First {
        // One server at a time, in order: formatting and rename must not run twice.
        let mut failures = Vec::new();
        for m in members.iter().filter(|m| offered(m)) {
            match send(m, params.clone())() {
                Ok(v) => return Ok(v),
                Err(e) => failures.push(format!("{}: {e}", m.name())),
            }
        }
        return if failures.is_empty() {
            Ok(Value::Null)
        } else {
            Err(failures.join("; "))
        };
    }
    let waits: Vec<(
        usize,
        String,
        Option<Box<dyn FnOnce() -> Result<Value, String> + Send>>,
    )> = members
        .iter()
        .enumerate()
        .map(|(i, m)| {
            let wait = offered(m).then(|| send(m, params.clone()));
            (i, m.name(), wait)
        })
        .collect();
    let answers = waits
        .into_iter()
        .map(|(server, name, wait)| Answer {
            server,
            name,
            outcome: match wait {
                None => Outcome::NotOffered,
                Some(w) => match w() {
                    Ok(v) => Outcome::Answered(v),
                    Err(e) => Outcome::Failed(e),
                },
            },
        })
        .collect();
    merge(method, answers)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn answer(server: usize, name: &str, v: Value) -> Answer {
        Answer {
            server,
            name: name.into(),
            outcome: Outcome::Answered(v),
        }
    }

    #[test]
    fn completion_lists_concatenate_with_their_servers_and_defaults() {
        let ts = json!({"isIncomplete": false, "items": [{"label": "log", "data": {"x": 1}}],
                        "itemDefaults": {"editRange": {"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 2}},
                                         "commitCharacters": ["."]}});
        let other = json!([{"label": "div", "labelDetails": {"description": "tag"}}]);
        let merged = merge(
            "textDocument/completion",
            vec![
                answer(0, "TypeScript", ts),
                Answer {
                    server: 1,
                    name: "ESLint".into(),
                    outcome: Outcome::NotOffered,
                },
                answer(2, "Emmet", other),
            ],
        )
        .unwrap();
        let items = merged["items"].as_array().unwrap();
        assert_eq!(items.len(), 2);
        assert_eq!(items[0]["labelDetails"]["description"], "TypeScript");
        assert_eq!(items[0]["data"], json!({ORIGIN: 0, "data": {"x": 1}}));
        assert_eq!(items[0]["commitCharacters"], json!(["."]));
        assert_eq!(items[0]["textEdit"]["newText"], "log");
        assert_eq!(items[1]["labelDetails"]["description"], "tag");
        assert_eq!(items[1]["data"], json!({ORIGIN: 2}));
        assert_eq!(merged["isIncomplete"], false);
        // Resolving goes back to the item's server with its own data.
        let (server, item) = route("completionItem/resolve", &items[0]).unwrap();
        assert_eq!((server, &item["data"]), (0, &json!({"x": 1})));
        let (server, item) = route("completionItem/resolve", &items[1]).unwrap();
        assert_eq!(server, 2);
        assert!(item.get("data").is_none());
        assert_eq!(
            retag("completionItem/resolve", 2, json!({"label": "div"}))["data"],
            json!({ORIGIN: 2})
        );
    }

    #[test]
    fn code_actions_concatenate_and_commands_find_their_server() {
        let ts = json!([{"title": "Add import", "kind": "quickfix", "data": {"id": 3}}]);
        let eslint = json!([{"title": "Fix this prefer-const problem", "kind": "quickfix",
                             "command": {"title": "Fix", "command": "eslint.applySingleFix", "arguments": []}}]);
        let merged = merge(
            "textDocument/codeAction",
            vec![answer(0, "TypeScript", ts), answer(1, "ESLint", eslint)],
        )
        .unwrap();
        let titles: Vec<&str> = merged
            .as_array()
            .unwrap()
            .iter()
            .map(|a| a["title"].as_str().unwrap())
            .collect();
        assert_eq!(titles, ["Add import", "Fix this prefer-const problem"]);
        assert_eq!(route("codeAction/resolve", &merged[1]).unwrap().0, 1);
        let caps = [
            json!({"executeCommandProvider": {"commands": ["_typescript.applyRefactoring"]}}),
            json!({"executeCommandProvider": {"commands": ["eslint.applySingleFix", "eslint.applyAllFixes"]}}),
        ];
        assert_eq!(
            command_owner("eslint.applyAllFixes", caps.iter().map(Some)),
            Some(1)
        );
        assert_eq!(command_owner("x.unknown", caps.iter().map(Some)), None);
    }

    #[test]
    fn hover_is_the_first_non_empty_and_formatting_the_first_answer() {
        let merged = merge(
            "textDocument/hover",
            vec![
                answer(
                    0,
                    "a",
                    json!({"contents": {"kind": "markdown", "value": ""}}),
                ),
                answer(1, "b", json!({"contents": "number"})),
            ],
        )
        .unwrap();
        assert_eq!(merged["contents"], "number");
        assert_eq!(
            merge("textDocument/hover", vec![answer(0, "a", Value::Null)]).unwrap(),
            Value::Null
        );
        let merged = merge(
            "textDocument/formatting",
            vec![
                Answer {
                    server: 0,
                    name: "a".into(),
                    outcome: Outcome::NotOffered,
                },
                answer(1, "b", json!([{"newText": "x"}])),
                answer(2, "c", json!([{"newText": "y"}])),
            ],
        )
        .unwrap();
        assert_eq!(merged[0]["newText"], "x");
    }

    #[test]
    fn locations_drop_duplicates_and_failures_only_fail_when_all_fail() {
        let loc = json!({"uri": "file:///a.ts", "range": {"start": {"line": 1, "character": 0}, "end": {"line": 1, "character": 3}}});
        let merged = merge(
            "textDocument/references",
            vec![
                answer(0, "a", json!([loc.clone()])),
                answer(1, "b", loc.clone()),
                Answer {
                    server: 2,
                    name: "c".into(),
                    outcome: Outcome::Failed("boom".into()),
                },
            ],
        )
        .unwrap();
        assert_eq!(merged, json!([loc]));
        let failed = merge(
            "textDocument/hover",
            vec![Answer {
                server: 0,
                name: "a".into(),
                outcome: Outcome::Failed("boom".into()),
            }],
        );
        assert_eq!(failed.unwrap_err(), "a: boom");
    }

    #[test]
    fn capabilities_decide_who_is_asked() {
        let ts = json!({"completionProvider": {"resolveProvider": true}, "hoverProvider": true,
                        "renameProvider": {"prepareProvider": true}, "codeActionProvider": true,
                        "documentFormattingProvider": true});
        let eslint = json!({"codeActionProvider": {"codeActionKinds": ["quickfix"]},
                            "executeCommandProvider": {"commands": ["eslint.applyAllFixes"]}});
        assert!(offers("textDocument/completion", &ts));
        assert!(!offers("textDocument/completion", &eslint));
        assert!(offers("completionItem/resolve", &ts));
        assert!(offers("textDocument/codeAction", &eslint));
        assert!(!offers("codeAction/resolve", &eslint));
        assert!(!offers("textDocument/formatting", &eslint));
        assert!(offers("textDocument/prepareRename", &ts));
        assert!(!offers(
            "textDocument/hover",
            &json!({"hoverProvider": false})
        ));
        assert!(offers("textDocument/diagnostic", &eslint));
    }
}
