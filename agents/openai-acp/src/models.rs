//! The model list: `GET {base}/models` and the person's catalog (`--catalog`), with the context windows the
//! servers report, and the ACP `model` config option built from them (brief 0058's shape).
//!
//! Status rules (as in the owner's slopcoder catalog, re-implemented): 401 and 403 mean the key was rejected; 400,
//! 404, 405, 410 and 501 mean the server does not list models (`listing: "none"`, or `"catalog"` when there is one);
//! any other non-2xx is an error with the status; a 200 whose body is not JSON (a proxy's landing page) is no
//! listing. A provider with a catalog never asks the server.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// One model: its id, the name the picker shows, and its context window (0: unknown).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelInfo {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub context_window: u64,
}

fn is_zero(n: &u64) -> bool {
    *n == 0
}

impl ModelInfo {
    pub fn new(id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            name: None,
            context_window: 0,
        }
    }

    /// The name the picker shows: the catalog's name, else the id.
    pub fn display_name(&self) -> &str {
        self.name.as_deref().unwrap_or(&self.id)
    }
}

/// Where a model list came from (`agents-provider-models.output.json`'s `listing`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ListingKind {
    Server,
    Catalog,
    None,
}

/// A model listing, as `eludite.agents.provider_models` answers it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Listing {
    pub models: Vec<ModelInfo>,
    pub listing: ListingKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    /// The key was rejected (401 or 403): the session cannot start.
    #[serde(skip)]
    pub key_rejected: bool,
    /// The server could not be reached or answered an unexpected error.
    #[serde(skip)]
    pub failed: bool,
}

impl Listing {
    fn none(message: impl Into<String>) -> Self {
        Self {
            models: Vec::new(),
            listing: ListingKind::None,
            message: Some(message.into()),
            key_rejected: false,
            failed: false,
        }
    }

    /// The catalog itself, or no listing with `message` when there is none.
    fn catalog_or_none(catalog: &[ModelInfo], message: impl Into<String>) -> Self {
        if catalog.is_empty() {
            Self::none(message)
        } else {
            Self::catalog(catalog)
        }
    }

    /// The person's catalog as the listing.
    pub fn catalog(catalog: &[ModelInfo]) -> Self {
        let mut models = catalog.to_vec();
        sort(&mut models);
        Self {
            models,
            listing: ListingKind::Catalog,
            message: None,
            key_rejected: false,
            failed: false,
        }
    }

    /// The JSON `eludite-openai-acp models` prints (`agents-provider-models.output.json`).
    pub fn to_json(&self) -> Value {
        serde_json::to_value(self).expect("listings serialize")
    }
}

/// Parse the `--catalog` value: a JSON array of `{id, name?, contextWindow?}`.
pub fn parse_catalog(text: &str) -> Result<Vec<ModelInfo>, String> {
    let models: Vec<ModelInfo> =
        serde_json::from_str(text).map_err(|e| format!("the model catalog is not valid: {e}"))?;
    if models.iter().any(|m| m.id.trim().is_empty()) {
        return Err("the model catalog has a model with an empty id".into());
    }
    Ok(models)
}

fn sort(models: &mut [ModelInfo]) {
    models.sort_by(|a, b| {
        a.id.to_lowercase()
            .cmp(&b.id.to_lowercase())
            .then(a.id.cmp(&b.id))
    });
}

/// The listing for a `GET {base}/models` answer: `status` and `body` (`None`: the request failed with that message
/// in `transport_error`). Context windows come from llama.cpp's `meta.n_ctx_train`, OpenRouter's `context_length`,
/// else the catalog's entry for the same id.
pub fn from_response(
    status: u16,
    body: &str,
    catalog: &[ModelInfo],
    transport_error: Option<&str>,
) -> Listing {
    if let Some(e) = transport_error {
        let mut l = Listing::none(format!("could not reach the server: {e}"));
        l.failed = true;
        return l;
    }
    match status {
        200..=299 => {}
        401 | 403 => {
            let mut l = Listing::none(format!("the key was rejected ({status})"));
            l.key_rejected = true;
            return l;
        }
        400 | 404 | 405 | 410 | 501 => {
            return Listing::catalog_or_none(catalog, "this server does not list models");
        }
        other => {
            let detail = error_message(body)
                .map(|m| format!(": {m}"))
                .unwrap_or_default();
            let mut l = Listing::none(format!("the server answered {other}{detail}"));
            l.failed = true;
            return l;
        }
    }
    let Ok(v) = serde_json::from_str::<Value>(body) else {
        return Listing::catalog_or_none(catalog, "the server's answer is not a model list");
    };
    let Some(data) = v.get("data").and_then(Value::as_array) else {
        return Listing::catalog_or_none(catalog, "the server's answer is not a model list");
    };
    let mut models: Vec<ModelInfo> = data
        .iter()
        .filter_map(|m| {
            let id = m.get("id")?.as_str()?.to_owned();
            let known = catalog.iter().find(|c| c.id == id);
            let window = m
                .pointer("/meta/n_ctx_train")
                .and_then(Value::as_u64)
                .or_else(|| m.get("context_length").and_then(Value::as_u64))
                .or_else(|| known.map(|c| c.context_window))
                .unwrap_or(0);
            Some(ModelInfo {
                name: known.and_then(|c| c.name.clone()),
                id,
                context_window: window,
            })
        })
        .collect();
    sort(&mut models);
    models.dedup_by(|a, b| a.id == b.id);
    Listing {
        message: models
            .is_empty()
            .then(|| "the server lists no models".to_owned()),
        models,
        listing: ListingKind::Server,
        key_rejected: false,
        failed: false,
    }
}

/// The server's error text from an error body: OpenAI's `error.message`, a plain `error` string, or `message`.
pub fn error_message(body: &str) -> Option<String> {
    let v: Value = serde_json::from_str(body).ok()?;
    let e = v.get("error").unwrap_or(&v);
    e.get("message")
        .and_then(Value::as_str)
        .or_else(|| e.as_str())
        .map(|s| s.chars().take(500).collect())
}

/// The ACP `model` config option (category `model`, a select) over `models`, `current` selected.
pub fn model_config_option(models: &[ModelInfo], current: &str) -> Value {
    let options: Vec<Value> = models
        .iter()
        .map(|m| json!({"value": m.id, "name": m.display_name()}))
        .collect();
    json!({
        "id": "model",
        "name": "Model",
        "description": "The model the next request uses",
        "category": "model",
        "type": "select",
        "currentValue": current,
        "options": options,
    })
}

/// The session's models: the listing, with `preferred` (`--model`) first when the server did not list it.
pub fn session_models(listed: &[ModelInfo], preferred: Option<&str>) -> Vec<ModelInfo> {
    let mut out = listed.to_vec();
    if let Some(p) = preferred
        && !out.iter().any(|m| m.id == p)
    {
        out.insert(0, ModelInfo::new(p));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn llama_cpp_openrouter_and_openai_shapes() {
        let llama = r#"{"object":"list","data":[{"id":"qwen3-8b-q4_k_m.gguf","object":"model","owned_by":"llamacpp","meta":{"n_ctx_train":40960,"n_params":8190735360}}]}"#;
        let l = from_response(200, llama, &[], None);
        assert_eq!(l.listing, ListingKind::Server);
        assert_eq!(l.models[0].context_window, 40_960);
        let router =
            r#"{"data":[{"id":"z/b","context_length":131072},{"id":"A/a","context_length":8192}]}"#;
        let l = from_response(200, router, &[], None);
        assert_eq!(
            l.models.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(),
            ["A/a", "z/b"],
            "sorted by id, case folded"
        );
        assert_eq!(l.models[1].context_window, 131_072);
        let openai =
            r#"{"object":"list","data":[{"id":"gpt-5","object":"model","owned_by":"openai"}]}"#;
        let catalog = vec![ModelInfo {
            id: "gpt-5".into(),
            name: Some("GPT-5".into()),
            context_window: 400_000,
        }];
        let l = from_response(200, openai, &[], None);
        assert_eq!(l.models[0].context_window, 0);
        let l = from_response(200, openai, &catalog, None);
        assert_eq!(l.models[0].context_window, 400_000);
        assert_eq!(l.models[0].display_name(), "GPT-5");
    }

    #[test]
    fn statuses() {
        let cat = vec![ModelInfo::new("m")];
        assert!(from_response(401, "", &[], None).key_rejected);
        assert_eq!(
            from_response(403, "", &[], None).message.as_deref(),
            Some("the key was rejected (403)")
        );
        for s in [400, 404, 405, 410, 501] {
            assert_eq!(from_response(s, "", &[], None).listing, ListingKind::None);
            assert_eq!(
                from_response(s, "", &cat, None).listing,
                ListingKind::Catalog
            );
        }
        let e = from_response(500, r#"{"error":{"message":"boom"}}"#, &[], None);
        assert!(e.failed);
        assert_eq!(e.message.as_deref(), Some("the server answered 500: boom"));
        let html = from_response(200, "<html>proxy</html>", &[], None);
        assert_eq!(html.listing, ListingKind::None);
        assert!(!html.failed);
        assert!(from_response(0, "", &[], Some("refused")).failed);
    }

    #[test]
    fn catalog_and_config_option() {
        let c =
            parse_catalog(r#"[{"id":"b","name":"Bee","contextWindow":16384},{"id":"a"}]"#).unwrap();
        assert_eq!(c[0].context_window, 16_384);
        assert!(parse_catalog(r#"[{"id":""}]"#).is_err());
        assert!(parse_catalog("{").is_err());
        let l = Listing::catalog(&c);
        assert_eq!(l.models[0].id, "a");
        assert_eq!(
            l.to_json(),
            json!({"models": [{"id": "a"}, {"id": "b", "name": "Bee", "contextWindow": 16384}], "listing": "catalog"})
        );
        let opt = model_config_option(&l.models, "b");
        assert_eq!(opt["currentValue"], "b");
        assert_eq!(opt["category"], "model");
        assert_eq!(opt["options"][1], json!({"value": "b", "name": "Bee"}));
        let s = session_models(&l.models, Some("x"));
        assert_eq!(s[0].id, "x");
        assert_eq!(session_models(&l.models, Some("a")).len(), 2);
    }
}
