//! The REST client every family uses: a base url, the authorization header, conditional requests against the cache
//! (`If-None-Match` with the last `ETag`, a 304 answered from the kept body), rate-limit headers honored (an
//! exhausted limit answers `rate_limited` with the reset time without sending), errors mapped to [`ErrorKind`]s, and
//! `Link` paging.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use serde_json::Value;

use crate::cache::{Cache, HttpEntry};
use crate::error::{ErrorKind, ForgeError, Result};
use crate::http::{Cancel, Method, Request, Response, Transport};
use crate::util::{now_secs, parse_time, rfc3339};

/// The reset times of exhausted rate limits, by host, shared by every client of a hub.
#[derive(Debug, Default, Clone)]
pub struct Limits(Arc<Mutex<HashMap<String, i64>>>);

impl Limits {
    fn exhausted_until(&self, host: &str) -> Option<i64> {
        let m = self.0.lock().unwrap_or_else(|e| e.into_inner());
        m.get(host).copied().filter(|t| *t > now_secs())
    }

    fn set(&self, host: &str, reset: Option<i64>) {
        let mut m = self.0.lock().unwrap_or_else(|e| e.into_inner());
        match reset {
            Some(t) => {
                m.insert(host.to_owned(), t);
            }
            None => {
                m.remove(host);
            }
        }
    }
}

/// A client for one forge host.
#[derive(Clone)]
pub struct Client {
    pub transport: Arc<dyn Transport>,
    /// The API's base url, without a trailing slash.
    pub base: String,
    /// The host it is for (messages, rate limits, sign-in).
    pub host: String,
    auth: Option<(String, String)>,
    headers: Vec<(String, String)>,
    cache: Option<Cache>,
    limits: Limits,
    pub cancel: Cancel,
}

impl std::fmt::Debug for Client {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Client")
            .field("base", &self.base)
            .field("host", &self.host)
            .field("signed_in", &self.auth.is_some())
            .finish_non_exhaustive()
    }
}

impl Client {
    pub fn new(
        transport: Arc<dyn Transport>,
        base: impl Into<String>,
        host: impl Into<String>,
    ) -> Self {
        Self {
            transport,
            base: base.into().trim_end_matches('/').to_owned(),
            host: host.into(),
            auth: None,
            headers: Vec::new(),
            cache: None,
            limits: Limits::default(),
            cancel: Cancel::new(),
        }
    }

    /// Send `name: value` as the authorization on every request (the value is never shown).
    pub fn with_auth(mut self, name: &str, value: String) -> Self {
        self.auth = Some((name.to_owned(), value));
        self
    }

    pub fn with_header(mut self, name: &str, value: &str) -> Self {
        self.headers.push((name.to_owned(), value.to_owned()));
        self
    }

    pub fn with_cache(mut self, cache: Option<Cache>) -> Self {
        self.cache = cache;
        self
    }

    pub fn with_limits(mut self, limits: Limits) -> Self {
        self.limits = limits;
        self
    }

    pub fn with_cancel(mut self, cancel: Cancel) -> Self {
        self.cancel = cancel;
        self
    }

    pub fn signed_in(&self) -> bool {
        self.auth.is_some()
    }

    /// `path` against the base (a full url is kept).
    pub fn url(&self, path: &str) -> String {
        if path.starts_with("http://") || path.starts_with("https://") {
            path.to_owned()
        } else if path.starts_with('/') {
            format!("{}{path}", self.base)
        } else {
            format!("{}/{path}", self.base)
        }
    }

    fn request(&self, method: Method, url: String) -> Request {
        let mut r = Request::new(method, url);
        for (k, v) in &self.headers {
            r = r.header(k, v.clone());
        }
        if let Some((k, v)) = &self.auth {
            r = r.header(k, v.clone());
        }
        r
    }

    /// Send `request` (with the auth and default headers added), honoring the rate limit; answers any HTTP status.
    pub fn send_raw(&self, mut request: Request) -> Result<Response> {
        self.cancel.check()?;
        if let Some(reset) = self.limits.exhausted_until(&self.host) {
            return Err(rate_limited(&self.host, reset));
        }
        let mut full = self.request(request.method, std::mem::take(&mut request.url));
        full.headers.extend(request.headers);
        full.body = request.body;
        let response = self.transport.send(&full, &self.cancel)?;
        if let Some(reset) = exhausted(&response) {
            self.limits.set(&self.host, Some(reset));
            if matches!(response.status, 403 | 429) {
                return Err(rate_limited(&self.host, reset));
            }
        }
        Ok(response)
    }

    /// GET `path` as JSON, conditionally: with the cache's last `ETag` the forge may answer 304 and the kept body is
    /// used. Answers the body and the `Link` header's next url.
    pub fn get_page(&self, path: &str) -> Result<(Value, Option<String>)> {
        let url = self.url(path);
        let kept = self.cache.as_ref().and_then(|c| c.http_get(&url));
        let mut req = Request::new(Method::Get, url.clone());
        if let Some(k) = &kept {
            if let Some(etag) = &k.etag {
                req = req.header("If-None-Match", etag.clone());
            } else if let Some(lm) = &k.last_modified {
                req = req.header("If-Modified-Since", lm.clone());
            }
        }
        let response = self.send_raw(req)?;
        if response.status == 304
            && let Some(k) = kept
        {
            let link = k
                .headers
                .iter()
                .find(|(n, _)| n.eq_ignore_ascii_case("link"))
                .and_then(|(_, v)| next_link(v));
            let value = serde_json::from_str(&k.body).unwrap_or(Value::Null);
            return Ok((value, link));
        }
        let value = self.check(&response)?;
        if let Some(cache) = &self.cache {
            let etag = response.header("etag").map(str::to_owned);
            let last_modified = response.header("last-modified").map(str::to_owned);
            if etag.is_some() || last_modified.is_some() {
                cache.http_put(&HttpEntry {
                    url: url.clone(),
                    etag,
                    last_modified,
                    fetched_at: now_secs(),
                    headers: keep_headers(&response),
                    body: response.text(),
                });
            }
        }
        Ok((value, response.header("link").and_then(next_link)))
    }

    /// GET `path` as JSON.
    pub fn get(&self, path: &str) -> Result<Value> {
        self.get_page(path).map(|(v, _)| v)
    }

    /// GET `path` and follow `Link: rel="next"` until `max` items or the last page; answers the items and the next
    /// page's url when stopped early.
    pub fn get_all(&self, path: &str, max: usize) -> Result<(Vec<Value>, Option<String>)> {
        let mut out = Vec::new();
        let mut next = Some(self.url(path));
        while let Some(url) = next.take() {
            self.cancel.check()?;
            let (v, link) = self.get_page(&url)?;
            match v {
                Value::Array(items) => out.extend(items),
                other => out.push(other),
            }
            next = link;
            if out.len() >= max {
                break;
            }
        }
        Ok((out, next))
    }

    /// Send JSON with `method` and answer the JSON response.
    pub fn send(&self, method: Method, path: &str, body: Option<&Value>) -> Result<Value> {
        let mut req = Request::new(method, self.url(path));
        if let Some(b) = body {
            req = req.json(b);
        }
        let response = self.send_raw(req)?;
        self.check(&response)
    }

    pub fn post(&self, path: &str, body: &Value) -> Result<Value> {
        self.send(Method::Post, path, Some(body))
    }

    pub fn patch(&self, path: &str, body: &Value) -> Result<Value> {
        self.send(Method::Patch, path, Some(body))
    }

    pub fn put(&self, path: &str, body: &Value) -> Result<Value> {
        self.send(Method::Put, path, Some(body))
    }

    /// GET text (a log), following redirects as the transport does.
    pub fn get_text(&self, path: &str) -> Result<String> {
        let response = self.send_raw(Request::new(Method::Get, self.url(path)))?;
        if response.status >= 400 {
            self.check(&response)?;
        }
        Ok(response.text())
    }

    /// The JSON of a successful response, or the error its status means.
    pub fn check(&self, response: &Response) -> Result<Value> {
        let html = response.body.iter().find(|b| !b.is_ascii_whitespace()) == Some(&b'<');
        match response.status {
            // Azure DevOps answers a refused token with 203 and its sign-in page; a redirect to a sign-in page
            // (Azure DevOps' anonymous reads of some resources) ends the same way.
            203 => Err(ForgeError::sign_in_required(
                &self.host,
                "the forge answered with its sign-in page.",
            )),
            200..=299 if html => Err(ForgeError::sign_in_required(
                &self.host,
                "the forge answered with a web page (its sign-in page) instead of data.",
            )),
            200..=299 => response.json(),
            status => Err(status_error(&self.host, status, response, self.signed_in())),
        }
    }
}

fn keep_headers(r: &Response) -> Vec<(String, String)> {
    r.headers
        .iter()
        .filter(|(k, _)| {
            let k = k.to_ascii_lowercase();
            k == "link" || k == "x-total" || k == "x-total-count" || k == "x-next-page"
        })
        .cloned()
        .collect()
}

/// The reset time when a response says the rate limit is exhausted.
fn exhausted(r: &Response) -> Option<i64> {
    let remaining = r
        .header("x-ratelimit-remaining")
        .or_else(|| r.header("ratelimit-remaining"))
        .and_then(|v| v.trim().parse::<i64>().ok());
    let reset = r
        .header("x-ratelimit-reset")
        .or_else(|| r.header("ratelimit-reset"))
        .and_then(|v| v.trim().parse::<i64>().ok())
        .map(|t| if t < 1_000_000_000 { now_secs() + t } else { t });
    let retry = r
        .header("retry-after")
        .and_then(|v| v.trim().parse::<i64>().ok())
        .map(|s| now_secs() + s);
    // The IETF draft's structured header, as Codeberg sends it: `RateLimit: "baseline";r=1999;t=600`.
    let (ietf_remaining, ietf_reset) = r
        .header("ratelimit")
        .map(|v| {
            let mut rem = None;
            let mut t = None;
            for p in v.split(';') {
                let p = p.trim();
                if let Some(x) = p.strip_prefix("r=") {
                    rem = x.trim().parse::<i64>().ok();
                } else if let Some(x) = p.strip_prefix("t=") {
                    t = x.trim().parse::<i64>().ok().map(|s| now_secs() + s);
                }
            }
            (rem, t)
        })
        .unwrap_or((None, None));
    let remaining = remaining.or(ietf_remaining);
    let reset = reset.or(ietf_reset);
    if r.status == 429 {
        return Some(retry.or(reset).unwrap_or(now_secs() + 60));
    }
    if remaining == Some(0) {
        return Some(reset.or(retry).unwrap_or(now_secs() + 60));
    }
    None
}

fn rate_limited(host: &str, reset: i64) -> ForgeError {
    let at = rfc3339(reset);
    ForgeError {
        kind: ErrorKind::RateLimited,
        message: format!("rate_limited: {host}'s API rate limit is exhausted until {at}"),
        host: Some(host.to_owned()),
        reset_at: Some(at),
    }
}

/// The forge's own message in an error body (`message`, `error`, `error_description`, `errors[0].message`).
pub fn forge_message(r: &Response) -> Option<String> {
    let v: Value = serde_json::from_slice(&r.body).ok()?;
    let pick = |v: &Value| -> Option<String> {
        for k in ["message", "error_description", "error", "msg"] {
            match v.get(k) {
                Some(Value::String(s)) if !s.is_empty() => return Some(s.clone()),
                Some(Value::Object(o)) => {
                    return Some(serde_json::to_string(o).unwrap_or_default());
                }
                Some(Value::Array(a)) if !a.is_empty() => {
                    return Some(
                        a.iter()
                            .map(|x| {
                                x.as_str()
                                    .map(str::to_owned)
                                    .unwrap_or_else(|| x.to_string())
                            })
                            .collect::<Vec<_>>()
                            .join("; "),
                    );
                }
                _ => {}
            }
        }
        None
    };
    let mut m = pick(&v)?;
    if let Some(errs) = v.get("errors").and_then(Value::as_array)
        && let Some(first) = errs.first()
    {
        let detail = first
            .get("message")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .unwrap_or_else(|| first.to_string());
        m = format!("{m}: {detail}");
    }
    Some(m)
}

/// The error an HTTP status means.
pub fn status_error(host: &str, status: u16, r: &Response, signed_in: bool) -> ForgeError {
    let said = forge_message(r).unwrap_or_else(|| format!("HTTP {status}"));
    let e = match status {
        401 => {
            return ForgeError::sign_in_required(
                host,
                if signed_in {
                    "the forge refused the stored token (expired or revoked)."
                } else {
                    "no token is stored for this host."
                },
            );
        }
        403 => ForgeError::new(ErrorKind::Forbidden, format!("forbidden: {said}")),
        404 if !signed_in => ForgeError::new(
            ErrorKind::NotFound,
            format!("not found: {said} (a private repository needs signing in)"),
        ),
        404 => ForgeError::new(ErrorKind::NotFound, format!("not found: {said}")),
        405 | 409 => ForgeError::new(ErrorKind::Conflict, format!("conflict: {said}")),
        400 | 422 => ForgeError::new(ErrorKind::Invalid, format!("the forge refused it: {said}")),
        _ => ForgeError::new(
            ErrorKind::Other,
            format!("{host} answered HTTP {status}: {said}"),
        ),
    };
    e.with_host(host)
}

/// The `rel="next"` url of a `Link` header.
pub fn next_link(link: &str) -> Option<String> {
    link.split(',').find_map(|part| {
        let mut pieces = part.split(';');
        let url = pieces
            .next()?
            .trim()
            .trim_start_matches('<')
            .trim_end_matches('>');
        pieces
            .any(|p| {
                let p = p.trim();
                p == "rel=\"next\"" || p == "rel=next"
            })
            .then(|| url.to_owned())
    })
}

/// A time member normalized to RFC 3339.
pub fn time_of(v: &Value, key: &str) -> Option<String> {
    v.get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty() && !s.starts_with("0001-01-01"))
        .map(crate::util::normalize_time)
}

/// Seconds between two forge times.
pub fn duration_between(start: Option<&str>, end: Option<&str>) -> Option<u64> {
    let (a, b) = (parse_time(start?)?, parse_time(end?)?);
    (b >= a).then_some((b - a) as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn link_headers() {
        let l = "<https://api.github.com/repositories/1/pulls?page=2>; rel=\"next\", \
                 <https://api.github.com/repositories/1/pulls?page=5>; rel=\"last\"";
        assert_eq!(
            next_link(l).as_deref(),
            Some("https://api.github.com/repositories/1/pulls?page=2")
        );
        assert_eq!(next_link("<x>; rel=\"prev\""), None);
    }

    #[test]
    fn rate_limit_headers() {
        let r = Response {
            status: 403,
            headers: vec![
                ("X-RateLimit-Remaining".into(), "0".into()),
                ("X-RateLimit-Reset".into(), "1900000000".into()),
            ],
            body: br#"{"message":"API rate limit exceeded"}"#.to_vec(),
        };
        assert_eq!(exhausted(&r), Some(1_900_000_000));
        let r = Response {
            status: 429,
            headers: vec![("Retry-After".into(), "30".into())],
            body: Vec::new(),
        };
        let t = exhausted(&r).unwrap();
        assert!((t - now_secs() - 30).abs() <= 2);
        let r = Response {
            status: 200,
            headers: vec![("RateLimit".into(), "\"baseline\";r=0;t=600".into())],
            body: Vec::new(),
        };
        let t = exhausted(&r).unwrap();
        assert!(
            (t - now_secs() - 600).abs() <= 2,
            "Codeberg's structured header"
        );
        let r = Response {
            status: 200,
            headers: vec![("RateLimit".into(), "\"baseline\";r=1999;t=600".into())],
            body: Vec::new(),
        };
        assert_eq!(exhausted(&r), None);
    }

    #[test]
    fn statuses_map_to_kinds() {
        let r = Response {
            status: 401,
            headers: vec![],
            body: br#"{"message":"Bad credentials"}"#.to_vec(),
        };
        let e = status_error("github.com", 401, &r, true);
        assert_eq!(e.kind, ErrorKind::SignInRequired);
        assert!(e.message.starts_with("sign_in_required: github.com"));
        let r = Response {
            status: 422,
            headers: vec![],
            body: br#"{"message":"Validation Failed","errors":[{"message":"A pull request already exists"}]}"#
                .to_vec(),
        };
        let e = status_error("github.com", 422, &r, true);
        assert_eq!(e.kind, ErrorKind::Invalid);
        assert!(e.message.contains("already exists"), "{e}");
    }
}
