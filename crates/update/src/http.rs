//! The HTTP layer: a [`Transport`] answers one GET, streaming the body into a writer so an archive of hundreds of
//! megabytes never sits in memory. [`UreqTransport`] is the real one (`ureq` 3 with `rustls`, the proxy from the
//! environment, `SSL_CERT_FILE` roots when set, timeouts); tests serve from a loopback server through it.

use std::io::Write;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use crate::error::{Error, Result};

/// One answer's status line and headers; the body went to the writer.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Answer {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    /// Bytes written to the sink.
    pub length: u64,
}

impl Answer {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

/// Called as the body arrives: bytes so far, the total when the answer said it.
pub type Progress<'a> = &'a dyn Fn(u64, Option<u64>);

/// Sends GETs.
pub trait Transport: Send + Sync {
    /// GET `url` with `headers`, writing the body to `sink` and reporting `progress`; `Err` only when no HTTP answer
    /// came (network, timeout, cancellation, or the sink failed).
    fn get(
        &self,
        url: &str,
        headers: &[(&str, &str)],
        sink: &mut dyn Write,
        progress: Progress<'_>,
        cancel: &Cancel,
    ) -> Result<Answer>;
}

/// A cancellation flag shared by a job and whoever may cancel it; checked before each request and between chunks.
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

    /// `Err(Canceled)` once canceled.
    pub fn check(&self) -> Result<()> {
        if self.is_canceled() {
            Err(Error::Canceled)
        } else {
            Ok(())
        }
    }
}

/// Seconds the whole of a small request (the release list, `SHA256SUMS`) may take.
pub const TIMEOUT_SECS: u64 = 30;
/// Seconds an archive download may take in all (a 200 MB tarball at 1 MB/s).
pub const DOWNLOAD_TIMEOUT_SECS: u64 = 30 * 60;

/// The real transport: `ureq` 3 over `rustls`.
pub struct UreqTransport {
    proxied: ureq::Agent,
    direct: ureq::Agent,
}

impl std::fmt::Debug for UreqTransport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("UreqTransport")
    }
}

impl Default for UreqTransport {
    fn default() -> Self {
        Self::new(Duration::from_secs(DOWNLOAD_TIMEOUT_SECS))
    }
}

impl UreqTransport {
    /// `timeout` bounds one whole request; the connect timeout is 10 s.
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

/// Whether `url`'s host is a loopback address (a test server; never proxied).
pub fn is_loopback(url: &str) -> bool {
    let host = host_of(url);
    let host = host.rsplit_once('@').map(|(_, h)| h).unwrap_or(&host);
    let host = if host.starts_with('[') {
        host.split(']').next().unwrap_or("").trim_start_matches('[')
    } else {
        host.split(':').next().unwrap_or("")
    };
    host == "localhost" || host == "::1" || host.starts_with("127.")
}

/// The host (and port) of a url.
pub fn host_of(url: &str) -> String {
    let rest = url.split("://").nth(1).unwrap_or(url);
    rest.split(['/', '?']).next().unwrap_or("").to_owned()
}

/// The chunk the body is copied in.
const CHUNK: usize = 64 * 1024;

impl Transport for UreqTransport {
    fn get(
        &self,
        url: &str,
        headers: &[(&str, &str)],
        sink: &mut dyn Write,
        progress: Progress<'_>,
        cancel: &Cancel,
    ) -> Result<Answer> {
        cancel.check()?;
        let agent = self.proxied_or_direct(is_loopback(url));
        let host = host_of(url);
        let network = |e: String| Error::Network {
            host: host.clone(),
            message: e,
        };
        let mut b = ureq::http::Request::builder().method("GET").uri(url);
        for (k, v) in headers {
            b = b.header(*k, *v);
        }
        let request = b.body(()).map_err(|e| network(e.to_string()))?;
        let mut response = agent.run(request).map_err(|e| network(e.to_string()))?;
        cancel.check()?;
        let status = response.status().as_u16();
        let headers: Vec<(String, String)> = response
            .headers()
            .iter()
            .map(|(k, v)| (k.as_str().to_owned(), v.to_str().unwrap_or("").to_owned()))
            .collect();
        let total = headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case("content-length"))
            .and_then(|(_, v)| v.trim().parse::<u64>().ok());
        let mut reader = response.body_mut().with_config().limit(u64::MAX).reader();
        let mut buf = vec![0u8; CHUNK];
        let mut length = 0u64;
        loop {
            cancel.check()?;
            let n =
                std::io::Read::read(&mut reader, &mut buf).map_err(|e| network(e.to_string()))?;
            if n == 0 {
                break;
            }
            sink.write_all(&buf[..n]).map_err(Error::Io)?;
            length += n as u64;
            progress(length, total);
        }
        sink.flush().map_err(Error::Io)?;
        Ok(Answer {
            status,
            headers,
            length,
        })
    }
}

impl UreqTransport {
    fn proxied_or_direct(&self, loopback: bool) -> &ureq::Agent {
        if loopback {
            &self.direct
        } else {
            &self.proxied
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loopback_is_recognized() {
        assert!(is_loopback("http://127.0.0.1:8080/x"));
        assert!(is_loopback("http://localhost/x"));
        assert!(is_loopback("http://[::1]:9/x"));
        assert!(!is_loopback("https://api.github.com/repos"));
        assert_eq!(
            host_of("https://api.github.com/repos?x=1"),
            "api.github.com"
        );
    }
}
