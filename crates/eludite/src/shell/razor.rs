//! Razor's HTML requests (brief 0079, ADR-0015): the host's `eludite/razor/html`, answered from the HTML language
//! server.
//!
//! `.razor` and `.cshtml` documents are served by Roslyn's Razor cohost extension through `eludite-host`, like C#.
//! For the HTML parts of a document (HTML completion inside markup, hover on an element, the markup's formatting)
//! Razor asks the client, and the host relays the question here with the document's HTML projection: the document
//! with its C# and Razor parts blanked out, so its positions are the document's.
//!
//! The shell keeps one virtual document per Razor document, `<Razor path>__virtual.html` (VS Code's name), opened in
//! the HTML server the `html` registration starts for that path ([`Shell::server_for_path`]) with `languageId`
//! `html`. A projection with a new checksum is sent as the next version's whole text; the same checksum sends
//! nothing. The request goes to the HTML server with `textDocument.uri` replaced by the virtual document's, and the
//! virtual uri in its answer is mapped back to the Razor document's. The answer is sent from the reply's own task:
//! the UI thread only queues the notifications and the request.
//!
//! `null` is the answer, at once, when the generation is stale, no HTML registration matches, or the HTML server is
//! down (not found: `tools/web-servers/fetch.sh`); and when the server fails or does not offer the method. Razor reads
//! `null` as "no HTML result", so C# and Razor features keep working without an HTML server. A virtual document whose
//! Razor document is no longer open is closed the next time a request arrives.

use std::collections::HashMap;
use std::path::PathBuf;

use eludite_lsp::Id;
use eludite_lsp::host::RazorHtmlParams;
use gpui::{AppContext as _, Context, Window};
use serde_json::Value;

use super::Shell;
use super::documents::{path_to_uri, trace, uri_to_path};
use super::servers::ServerKey;
use super::session::ServerSession;

/// The virtual HTML documents of the Razor documents, by Razor uri.
#[derive(Default)]
pub struct RazorHtml {
    virtual_docs: HashMap<String, VirtualDocument>,
    /// `eludite/razor/html` requests answered (tests and the log).
    pub answered: u64,
}

struct VirtualDocument {
    uri: String,
    session: ServerSession,
    checksum: String,
    version: i32,
}

/// The virtual HTML document's path for a Razor document at `path`.
pub fn virtual_path(path: &std::path::Path) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(VIRTUAL_SUFFIX);
    PathBuf::from(name)
}

/// Whether `uri` names a virtual HTML document (a server's diagnostics for it are not shown).
pub fn is_virtual_uri(uri: &str) -> bool {
    uri.ends_with(VIRTUAL_SUFFIX)
}

const VIRTUAL_SUFFIX: &str = "__virtual.html";

/// `params` with `textDocument.uri` replaced by `uri` (when it has one).
fn retarget(mut params: Value, uri: &str) -> Value {
    if let Some(doc) = params.get_mut("textDocument").and_then(Value::as_object_mut)
        && doc.contains_key("uri")
    {
        doc.insert("uri".into(), Value::String(uri.to_owned()));
    }
    params
}

/// Every string in `value` equal to `from` becomes `to` (locations, workspace-edit keys and `textDocument`s alike).
pub fn map_uri(value: Value, from: &str, to: &str) -> Value {
    match value {
        Value::String(s) if s == from => Value::String(to.to_owned()),
        Value::Array(items) => Value::Array(items.into_iter().map(|v| map_uri(v, from, to)).collect()),
        Value::Object(map) => Value::Object(
            map.into_iter()
                .map(|(k, v)| {
                    let k = if k == from { to.to_owned() } else { k };
                    (k, map_uri(v, from, to))
                })
                .collect(),
        ),
        other => other,
    }
}

impl Shell {
    /// The host's `eludite/razor/html` request `id`.
    pub(super) fn on_razor_html(
        &mut self,
        id: Id,
        params: RazorHtmlParams,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.close_stale_virtual_documents();
        let host = self.session.clone();
        self.razor.answered += 1;
        if params.generation != self.generation {
            trace(format_args!(
                "razor/html {}: stale generation {} (current {})",
                params.method, params.generation, self.generation
            ));
            host.respond_razor_html(id, Value::Null);
            return;
        }
        let Some(session) = self.virtual_document(&params, window, cx) else {
            host.respond_razor_html(id, Value::Null);
            return;
        };
        let virtual_uri = self.razor.virtual_docs[&params.uri].uri.clone();
        let razor_uri = params.uri.clone();
        let (_, rx) = session.request_value(&params.method, retarget(params.params, &virtual_uri));
        let method = params.method;
        cx.background_spawn(async move {
            let result = match rx.await {
                Ok(reply) => match reply.result {
                    Ok(v) => map_uri(v, &virtual_uri, &razor_uri),
                    Err(e) => {
                        trace(format_args!("razor/html {method}: {e:?}"));
                        Value::Null
                    }
                },
                Err(_) => Value::Null,
            };
            host.respond_razor_html(id, result);
        })
        .detach();
    }

    /// The HTML server's session for the request's Razor document, its virtual document opened or brought to the
    /// request's projection; `None` when there is no HTML server for it.
    fn virtual_document(
        &mut self,
        params: &RazorHtmlParams,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<ServerSession> {
        if let Some(doc) = self.razor.virtual_docs.get_mut(&params.uri) {
            if doc.checksum != params.checksum {
                doc.checksum.clone_from(&params.checksum);
                doc.version += 1;
                doc.session
                    .did_change(doc.uri.clone(), doc.version, params.text.clone());
            }
            let session = doc.session.clone();
            return self.html_server_up(&params.uri).then_some(session);
        }
        let path = virtual_path(&uri_to_path(&params.uri)?);
        let (keys, session, language_id) = self.server_for_path(&path, window, cx)?;
        if !keys
            .iter()
            .all(|k| matches!(k, ServerKey::Generic(_)))
        {
            return None;
        }
        let uri = path_to_uri(&path);
        session.did_open(uri.clone(), &language_id, 1, params.text.clone());
        self.razor.virtual_docs.insert(
            params.uri.clone(),
            VirtualDocument {
                uri,
                session: session.clone(),
                checksum: params.checksum.clone(),
                version: 1,
            },
        );
        self.html_server_up(&params.uri).then_some(session)
    }

    /// Whether the HTML server of the Razor document's virtual document can answer (not `not found` or exited).
    fn html_server_up(&self, razor_uri: &str) -> bool {
        let Some(path) = uri_to_path(razor_uri).map(|p| virtual_path(&p)) else {
            return false;
        };
        self.generic
            .values()
            .filter(|g| g.registration.matches(&path) && path.starts_with(&g.root))
            .any(|g| !g.down())
    }

    /// Closes the virtual documents whose Razor documents the shell no longer has open.
    fn close_stale_virtual_documents(&mut self) {
        let open: Vec<String> = self.documents.values().map(|d| d.uri.clone()).collect();
        let open_paths: Vec<PathBuf> = open.iter().filter_map(|u| uri_to_path(u)).collect();
        self.razor.virtual_docs.retain(|razor_uri, doc| {
            let still = open.iter().any(|u| u == razor_uri)
                || uri_to_path(razor_uri).is_some_and(|p| open_paths.contains(&p));
            if !still {
                doc.session.did_close(doc.uri.clone());
            }
            still
        });
    }

    /// The Razor HTML state (tests).
    #[cfg(test)]
    pub(super) fn razor_html(&self) -> &RazorHtml {
        &self.razor
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn the_virtual_document_sits_beside_the_razor_document() {
        assert_eq!(
            virtual_path(std::path::Path::new("/w/Pages/Home.razor")),
            PathBuf::from("/w/Pages/Home.razor__virtual.html")
        );
    }

    #[test]
    fn requests_are_retargeted_and_answers_mapped_back() {
        let params = retarget(
            json!({"textDocument": {"uri": "file:///a.razor"}, "position": {"line": 1, "character": 2}}),
            "file:///a.razor__virtual.html",
        );
        assert_eq!(params["textDocument"]["uri"], "file:///a.razor__virtual.html");
        assert_eq!(params["position"]["line"], 1);
        // No textDocument (completionItem/resolve): unchanged.
        assert_eq!(retarget(json!({"label": "div"}), "x"), json!({"label": "div"}));

        let answer = map_uri(
            json!([{"uri": "file:///a.razor__virtual.html", "range": {}},
                   {"changes": {"file:///a.razor__virtual.html": [{"newText": "file:///a.razor__virtual.html!"}]}}]),
            "file:///a.razor__virtual.html",
            "file:///a.razor",
        );
        assert_eq!(answer[0]["uri"], "file:///a.razor");
        assert!(answer[1]["changes"]["file:///a.razor"].is_array());
        // Only whole strings are uris: text that merely contains one is left alone.
        assert_eq!(
            answer[1]["changes"]["file:///a.razor"][0]["newText"],
            "file:///a.razor__virtual.html!"
        );
    }
}
