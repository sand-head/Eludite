//! The HTTP layer: a [`Transport`] sends one [`Request`] and answers its [`Response`]. [`UreqTransport`] is the real
//! one (`ureq` with `rustls`, the proxy from the environment, `SSL_CERT_FILE` roots when set, timeouts); the
//! [`crate::replay`] transport answers from recorded fixtures, so every call can run without a network.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use crate::error::{ErrorKind, ForgeError, Result};

/// An HTTP method.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Method {
    Get,
    Post,
    Put,
    Patch,
    Delete,
}

impl Method {
    pub fn as_str(self) -> &'static str {
        match self {
            Method::Get => "GET",
            Method::Post => "POST",
            Method::Put => "PUT",
            Method::Patch => "PATCH",
            Method::Delete => "DELETE",
        }
    }

    pub fn parse(s: &str) -> Option<Method> {
        Some(match s.to_ascii_uppercase().as_str() {
            "GET" => Method::Get,
            "POST" => Method::Post,
            "PUT" => Method::Put,
            "PATCH" => Method::Patch,
            "DELETE" => Method::Delete,
            _ => return None,
        })
    }
}

/// One request. Its `Debug` hides the `Authorization` header's value.
#[derive(Clone, PartialEq, Eq)]
pub struct Request {
    pub method: Method,
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: Option<Vec<u8>>,
}

impl std::fmt::Debug for Request {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let headers: Vec<(String, String)> = self
            .headers
            .iter()
            .map(|(k, v)| {
                if is_secret_header(k) {
                    (k.clone(), "<hidden>".to_owned())
                } else {
                    (k.clone(), v.clone())
                }
            })
            .collect();
        f.debug_struct("Request")
            .field("method", &self.method)
            .field("url", &self.url)
            .field("headers", &headers)
            .field("body_len", &self.body.as_ref().map(Vec::len))
            .finish()
    }
}

/// Headers that carry a secret, hidden wherever a request is shown.
pub fn is_secret_header(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    n == "authorization" || n == "private-token" || n == "cookie" || n.contains("token")
}

impl Request {
    pub fn new(method: Method, url: impl Into<String>) -> Self {
        Self {
            method,
            url: url.into(),
            headers: Vec::new(),
            body: None,
        }
    }

    pub fn header(mut self, name: &str, value: impl Into<String>) -> Self {
        self.headers.push((name.to_owned(), value.into()));
        self
    }

    pub fn json(mut self, body: &serde_json::Value) -> Self {
        self.body = Some(serde_json::to_vec(body).expect("serializes"));
        self.headers
            .push(("Content-Type".into(), "application/json".into()));
        self
    }

    pub fn form(mut self, pairs: &[(&str, String)]) -> Self {
        self.body = Some(crate::util::query(pairs).into_bytes());
        self.headers.push((
            "Content-Type".into(),
            "application/x-www-form-urlencoded".into(),
        ));
        self
    }

    pub fn get_header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

/// One response.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Response {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Response {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }

    pub fn json(&self) -> Result<serde_json::Value> {
        if self.body.is_empty() {
            return Ok(serde_json::Value::Null);
        }
        serde_json::from_slice(&self.body).map_err(|e| {
            ForgeError::other(format!(
                "the forge answered something that is not JSON ({e}): {}",
                truncate(&self.text(), 200)
            ))
        })
    }
}

fn truncate(s: &str, n: usize) -> String {
    if s.len() <= n {
        s.to_owned()
    } else {
        let mut end = n;
        while !s.is_char_boundary(end) {
            end -= 1;
        }
        format!("{}…", &s[..end])
    }
}

/// Sends requests.
pub trait Transport: Send + Sync {
    /// Send `request`; `Err` only when no HTTP answer came (network, timeout, cancellation).
    fn send(&self, request: &Request, cancel: &Cancel) -> Result<Response>;
}

/// A cancellation flag shared by a call and whoever may cancel it; checked before each request and between pages.
#[derive(Debug, Clone, Default)]
pub struct Cancel(Arc<AtomicBool>);

impl Cancel {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cancel(&self) {
        self.0.store(true, Ordering::SeqCst);
    }

    pub fn is_canceled(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }

    /// `Err(canceled)` once canceled.
    pub fn check(&self) -> Result<()> {
        if self.is_canceled() {
            Err(ForgeError::canceled())
        } else {
            Ok(())
        }
    }
}

/// The real transport: `ureq` 3 over `rustls`.
pub struct UreqTransport {
    proxied: ureq::Agent,
    direct: ureq::Agent,
}

/// Seconds a request may take, whole.
pub const TIMEOUT_SECS: u64 = 30;

impl Default for UreqTransport {
    fn default() -> Self {
        Self::new(Duration::from_secs(TIMEOUT_SECS))
    }
}

impl UreqTransport {
    pub fn new(timeout: Duration) -> Self {
        let tls = || {
            let mut b = ureq::tls::TlsConfig::builder();
            if let Some(roots) = env_roots() {
                b = b.root_certs(roots);
            }
            b.build()
        };
        let build = |proxy: Option<ureq::Proxy>| -> ureq::Agent {
            ureq::Agent::config_builder()
                .http_status_as_error(false)
                .timeout_global(Some(timeout))
                .timeout_connect(Some(Duration::from_secs(10)))
                .max_redirects(5)
                .proxy(proxy)
                .tls_config(tls())
                .user_agent(concat!("Eludite/", env!("CARGO_PKG_VERSION")))
                .build()
                .into()
        };
        Self {
            proxied: build(ureq::Proxy::try_from_env()),
            direct: build(None),
        }
    }
}

/// `SSL_CERT_FILE`'s certificates as the roots, when it names a readable PEM bundle (a proxy that re-signs TLS).
fn env_roots() -> Option<ureq::tls::RootCerts> {
    let path = std::env::var_os("SSL_CERT_FILE")?;
    let pem = std::fs::read(path).ok()?;
    let certs: Vec<ureq::tls::Certificate<'static>> = ureq::tls::parse_pem(&pem)
        .filter_map(|item| match item {
            Ok(ureq::tls::PemItem::Certificate(c)) => Some(c),
            _ => None,
        })
        .collect();
    (!certs.is_empty()).then(|| ureq::tls::RootCerts::new_with_certs(&certs))
}

/// Whether `url`'s host is a loopback address (the fixture server; never proxied).
pub fn is_loopback(url: &str) -> bool {
    let rest = url.split("://").nth(1).unwrap_or(url);
    let host = rest.split(['/', '?']).next().unwrap_or("");
    let host = host.rsplit_once('@').map(|(_, h)| h).unwrap_or(host);
    let host = if host.starts_with('[') {
        host.split(']').next().unwrap_or("").trim_start_matches('[')
    } else {
        host.split(':').next().unwrap_or("")
    };
    host == "localhost" || host == "::1" || host.starts_with("127.")
}

/// The largest response body read (a log is paged by the caller).
pub const MAX_BODY: u64 = 32 * 1024 * 1024;

impl Transport for UreqTransport {
    fn send(&self, request: &Request, cancel: &Cancel) -> Result<Response> {
        cancel.check()?;
        let agent = if is_loopback(&request.url) {
            &self.direct
        } else {
            &self.proxied
        };
        let mut b = ureq::http::Request::builder()
            .method(request.method.as_str())
            .uri(&request.url);
        for (k, v) in &request.headers {
            b = b.header(k.as_str(), v.as_str());
        }
        let host = host_of(&request.url);
        let network = |e: String| {
            ForgeError::new(
                ErrorKind::Network,
                format!("network: {host} could not be reached ({e})"),
            )
            .with_host(host.clone())
        };
        let result = match &request.body {
            Some(body) => agent.run(b.body(body.clone()).map_err(|e| network(e.to_string()))?),
            None => agent.run(b.body(()).map_err(|e| network(e.to_string()))?),
        };
        let mut response = result.map_err(|e| network(e.to_string()))?;
        cancel.check()?;
        let status = response.status().as_u16();
        let headers = response
            .headers()
            .iter()
            .map(|(k, v)| (k.as_str().to_owned(), v.to_str().unwrap_or("").to_owned()))
            .collect();
        let body = response
            .body_mut()
            .with_config()
            .limit(MAX_BODY)
            .read_to_vec()
            .map_err(|e| network(e.to_string()))?;
        Ok(Response {
            status,
            headers,
            body,
        })
    }
}

/// The host (and port) of a url.
pub fn host_of(url: &str) -> String {
    let rest = url.split("://").nth(1).unwrap_or(url);
    rest.split(['/', '?']).next().unwrap_or("").to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_request_never_shows_its_token() {
        let r = Request::new(Method::Get, "https://api.github.com/user")
            .header("Authorization", "Bearer ghp_secret")
            .header("PRIVATE-TOKEN", "glpat-secret");
        let shown = format!("{r:?}");
        assert!(!shown.contains("secret"), "{shown}");
        assert!(shown.contains("<hidden>"));
    }

    #[test]
    fn loopback_hosts() {
        assert!(is_loopback("http://127.0.0.1:4000/x"));
        assert!(is_loopback("http://localhost/x"));
        assert!(is_loopback("http://[::1]:80/"));
        assert!(!is_loopback("https://codeberg.org/api/v1"));
        assert_eq!(
            host_of("https://gitlab.example.com:8443/api/v4"),
            "gitlab.example.com:8443"
        );
    }

    #[test]
    fn cancel_stops_before_sending() {
        let c = Cancel::new();
        c.cancel();
        let t = UreqTransport::default();
        let e = t
            .send(&Request::new(Method::Get, "http://127.0.0.1:9/"), &c)
            .unwrap_err();
        assert_eq!(e.kind, ErrorKind::Canceled);
    }
}
