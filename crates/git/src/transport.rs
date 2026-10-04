//! How a transfer reaches its remote (brief 0045): the proxy, the certificate rule and the refusals that name the
//! host.
//!
//! - **Proxy.** For an `https://` remote: `http.proxy` from git config (an empty value means none, as in git), else
//!   `HTTPS_PROXY`, `https_proxy`, then `HTTP_PROXY` from the environment, else none. A host `NO_PROXY` (or
//!   `no_proxy`) names, and a loopback host, is reached directly. libgit2 1.9 tunnels only https through a proxy
//!   (CONNECT): it sends plain `http://` straight to the server whatever the proxy, so none is given for those; ssh,
//!   `git://` and local remotes never use one. [`proxy_for`] takes the config value and the environment as
//!   arguments, so the order is tested with fakes.
//! - **Certificates.** libgit2's own check (OpenSSL on Linux, SecureTransport on macOS, WinHTTP and Schannel on
//!   Windows; `~/.ssh/known_hosts` for ssh), except that `http.sslVerify = false` in the repository's (or the
//!   global) config accepts any TLS certificate, as git does. A refused certificate names the host
//!   ([`certificate_refusal`]).

use crate::credentials::{remote_host, remote_scheme};
use crate::{ErrorKind, GitError};

/// The proxy for `url`: `configured` (git's `http.proxy`; empty: none), else the environment's `env(name)`, else
/// none; `NO_PROXY` and loopback hosts go direct.
pub fn proxy_for(
    url: &str,
    configured: Option<&str>,
    env: &dyn Fn(&str) -> Option<String>,
) -> Option<String> {
    let scheme = remote_scheme(url);
    if scheme != "https" {
        return None;
    }
    let host = remote_host(url)?;
    let bare = host_without_port(&host);
    if is_loopback(bare) {
        return None;
    }
    let no_proxy = env("NO_PROXY")
        .filter(|v| !v.is_empty())
        .or_else(|| env("no_proxy"));
    if let Some(list) = no_proxy
        && no_proxy_matches(&list, bare)
    {
        return None;
    }
    if let Some(c) = configured {
        let c = c.trim();
        return (!c.is_empty()).then(|| c.to_owned());
    }
    ["HTTPS_PROXY", "https_proxy", "HTTP_PROXY"]
        .iter()
        .find_map(|n| {
            env(n)
                .map(|v| v.trim().to_owned())
                .filter(|v| !v.is_empty())
        })
}

/// `host` without its port (`[::1]:8443` gives `::1`).
fn host_without_port(host: &str) -> &str {
    if let Some(rest) = host.strip_prefix('[') {
        return rest.split(']').next().unwrap_or(rest);
    }
    match host.rsplit_once(':') {
        Some((h, p)) if !h.contains(':') && p.chars().all(|c| c.is_ascii_digit()) => h,
        _ => host,
    }
}

fn is_loopback(host: &str) -> bool {
    host.eq_ignore_ascii_case("localhost")
        || host
            .parse::<std::net::IpAddr>()
            .is_ok_and(|ip| ip.is_loopback())
}

/// Whether `NO_PROXY`'s `list` names `host`: `*`, the host, a domain suffix (`example.com` and `.example.com` both
/// match `git.example.com`), an address, or an IPv4 range (`10.0.0.0/8`).
pub fn no_proxy_matches(list: &str, host: &str) -> bool {
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    let ip: Option<std::net::IpAddr> = host.parse().ok();
    list.split([',', ' '])
        .map(str::trim)
        .filter(|e| !e.is_empty())
        .any(|entry| {
            let entry = entry.to_ascii_lowercase();
            if entry == "*" {
                return true;
            }
            if let Some((net, bits)) = entry.split_once('/') {
                return match (ip, net.parse::<std::net::IpAddr>(), bits.parse::<u32>()) {
                    (
                        Some(std::net::IpAddr::V4(a)),
                        Ok(std::net::IpAddr::V4(n)),
                        Ok(bits @ 0..=32),
                    ) => {
                        let mask = if bits == 0 {
                            0
                        } else {
                            u32::MAX << (32 - bits)
                        };
                        u32::from(a) & mask == u32::from(n) & mask
                    }
                    (Some(std::net::IpAddr::V6(a)), Ok(std::net::IpAddr::V6(n)), Ok(128)) => a == n,
                    _ => false,
                };
            }
            let entry = entry.trim_start_matches("*.").trim_start_matches('.');
            let entry = host_without_port(entry);
            host == entry || host.ends_with(&format!(".{entry}"))
        })
}

/// The refusal of a TLS certificate (or an ssh host key) `host` presented; `detail` is libgit2's message.
pub fn certificate_refusal(host: &str, ssh: bool, detail: &str) -> GitError {
    let message = if ssh {
        format!(
            "The ssh host key of {host} is not trusted ({detail}): it is not in ~/.ssh/known_hosts, or it changed. \
             Connect once with `ssh` to check the key and accept it, then try again."
        )
    } else {
        format!(
            "The certificate of {host} could not be verified ({detail}), so the connection was refused. Trust the \
             certificate authority that issued it on this machine; for a server of your own with a self-signed \
             certificate, `git config http.sslVerify false` in this repository turns the check off (Eludite honors \
             it, as git does, and warns while it is set)."
        )
    };
    let mut e = GitError::new(ErrorKind::Certificate, message);
    e.host = Some(host.to_owned());
    e
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn env(vars: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let map: HashMap<String, String> = vars
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        move |k| map.get(k).cloned()
    }

    const URL: &str = "https://git.example.com/r.git";

    #[test]
    fn the_proxy_comes_from_http_proxy_then_the_environment_then_none() {
        let all = env(&[
            ("HTTPS_PROXY", "http://upper:1"),
            ("https_proxy", "http://lower:2"),
            ("HTTP_PROXY", "http://plain:3"),
        ]);
        // git config's http.proxy first.
        assert_eq!(
            proxy_for(URL, Some("http://config:8080"), &all).as_deref(),
            Some("http://config:8080")
        );
        // An empty http.proxy turns the proxy off, as in git.
        assert_eq!(proxy_for(URL, Some(""), &all), None);
        // Then HTTPS_PROXY, https_proxy, HTTP_PROXY in that order.
        assert_eq!(
            proxy_for(URL, None, &all).as_deref(),
            Some("http://upper:1")
        );
        let two = env(&[
            ("https_proxy", "http://lower:2"),
            ("HTTP_PROXY", "http://plain:3"),
        ]);
        assert_eq!(
            proxy_for(URL, None, &two).as_deref(),
            Some("http://lower:2")
        );
        let one = env(&[("HTTP_PROXY", "http://plain:3"), ("HTTPS_PROXY", "")]);
        assert_eq!(
            proxy_for(URL, None, &one).as_deref(),
            Some("http://plain:3")
        );
        // Else none.
        assert_eq!(proxy_for(URL, None, &env(&[])), None);
        // Plain http (libgit2 sends it straight to the server), ssh, git:// and local remotes never use one.
        for url in [
            "http://git.example.com/r.git",
            "ssh://git@git.example.com/r.git",
            "git@git.example.com:r.git",
            "git://h/r",
            "/tmp/r.git",
        ] {
            assert_eq!(
                proxy_for(url, Some("http://config:8080"), &all),
                None,
                "{url}"
            );
        }
    }

    #[test]
    fn no_proxy_and_loopback_hosts_go_direct() {
        let vars = env(&[
            ("HTTPS_PROXY", "http://proxy:1"),
            (
                "NO_PROXY",
                "localhost,.internal.example, corp.example ,10.0.0.0/8,[::2]",
            ),
        ]);
        for (url, direct) in [
            ("https://127.0.0.1:4443/r.git", true),
            ("https://localhost/r.git", true),
            ("https://[::1]:8443/r.git", true),
            ("https://a.internal.example/r.git", true),
            ("https://corp.example/r.git", true),
            ("https://git.corp.example:8443/r.git", true),
            ("https://10.1.2.3/r.git", true),
            ("https://11.1.2.3/r.git", false),
            ("https://notcorp.example/r.git", false),
            ("https://github.com/o/r.git", false),
        ] {
            let p = proxy_for(url, None, &vars);
            assert_eq!(p.is_none(), direct, "{url}: {p:?}");
            // NO_PROXY applies to http.proxy too, as curl applies it.
            assert_eq!(
                proxy_for(url, Some("http://c:2"), &vars).is_none(),
                direct,
                "{url}"
            );
        }
        assert!(no_proxy_matches("*", "anything"));
        let lower = env(&[
            ("HTTPS_PROXY", "http://proxy:1"),
            ("no_proxy", "github.com"),
        ]);
        assert_eq!(proxy_for("https://github.com/o/r", None, &lower), None);
    }

    #[test]
    fn refusals_name_the_host() {
        let e = certificate_refusal("127.0.0.1:4443", false, "the SSL certificate is invalid");
        assert_eq!(e.kind, ErrorKind::Certificate);
        assert_eq!(e.host.as_deref(), Some("127.0.0.1:4443"));
        assert!(
            e.message
                .starts_with("The certificate of 127.0.0.1:4443 could not be verified"),
            "{e}"
        );
        assert!(e.message.contains("http.sslVerify false"), "{e}");
        let e = certificate_refusal("example.com", true, "unknown host key");
        assert!(e.message.contains("ssh host key of example.com"), "{e}");
    }
}
