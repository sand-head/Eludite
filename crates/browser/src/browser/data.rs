//! What a site keeps and what it was sent (brief 0024): `storage`, `network_body`, and `open_external`.
//!
//! - **`storage`** reads cookies with `Network.getCookies` for the origin's url (the cookies the browser would send
//!   there) and `localStorage` and `sessionStorage` with `DOMStorage.getDOMStorageItems`; `clear` deletes exactly
//!   those cookies (`Network.deleteCookies` by name, domain and path) and the two storages (`DOMStorage.clear`).
//!   Cookies are read field by field, so a Chrome older or newer than the pinned protocol still answers.
//! - **`network_body`** is `Network.getResponseBody` of a request in the tab's ring. Chrome keeps bodies within the
//!   buffers `Network.enable` set when the tab was adopted ([`super::RESOURCE_BUFFER`], [`super::TOTAL_BUFFER`]); a
//!   body it no longer has, or never had, fails the command and says so.
//! - **`open_external`** starts the system's opener (`xdg-open`, `open`, `cmd /c start`), or `ELUDITE_OPENER` when
//!   set, or the program [`Browser::set_opener`] gave (tests), and never waits for it: a thread reaps it.

use std::collections::BTreeMap;

use eludite_commands::CommandError;
use eludite_commands::browser::{
    Cleared, CookieRow, NetworkBodyOutput, OpenExternalOutput, StorageAction, StorageItem,
    StorageKind, StorageOutput, cut,
};
use eludite_commands::policy::ParsedUrl;
use eludite_protocol::cdp::dom_storage;
use serde_json::{Value, json};

use super::{Browser, Tab, failed, invalid};
use crate::process::NoConsoleWindow as _;

/// The environment variable naming a program to open urls with instead of the system's (tests).
pub const OPENER_ENV: &str = "ELUDITE_OPENER";

/// The command line that opens `url` in the system browser: `program` ([`Browser::set_opener`]), else
/// `ELUDITE_OPENER` when set, else the platform's.
pub fn opener(url: &str, program: Option<&str>) -> (String, Vec<String>) {
    if let Some(p) = program {
        return (p.to_owned(), vec![url.to_owned()]);
    }
    if let Some(p) = std::env::var_os(OPENER_ENV).filter(|p| !p.is_empty()) {
        return (p.to_string_lossy().into_owned(), vec![url.to_owned()]);
    }
    if cfg!(target_os = "windows") {
        (
            "cmd".into(),
            vec!["/c".into(), "start".into(), String::new(), url.to_owned()],
        )
    } else if cfg!(target_os = "macos") {
        ("open".into(), vec![url.to_owned()])
    } else {
        ("xdg-open".into(), vec![url.to_owned()])
    }
}

/// Standard base64 with padding.
pub fn base64_encode(bytes: &[u8]) -> String {
    const A: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for c in bytes.chunks(3) {
        let n = (u32::from(c[0]) << 16)
            | (u32::from(*c.get(1).unwrap_or(&0)) << 8)
            | u32::from(*c.get(2).unwrap_or(&0));
        out.push(A[(n >> 18) as usize & 63] as char);
        out.push(A[(n >> 12) as usize & 63] as char);
        out.push(if c.len() > 1 {
            A[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if c.len() > 2 {
            A[n as usize & 63] as char
        } else {
            '='
        });
    }
    out
}

/// Decode standard base64 (whitespace and padding ignored); `None` on another character.
pub fn base64_decode(s: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(s.len() / 4 * 3);
    let (mut acc, mut bits) = (0u32, 0u32);
    for c in s.bytes() {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            b'=' | b'\n' | b'\r' | b' ' => continue,
            _ => return None,
        };
        acc = (acc << 6) | u32::from(v);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    Some(out)
}

/// Media types whose bodies are text.
fn is_text_type(mime: &str) -> bool {
    let m = mime.to_ascii_lowercase();
    m.starts_with("text/")
        || m.contains("json")
        || m.contains("javascript")
        || m.contains("ecmascript")
        || m.contains("xml")
        || m.contains("x-www-form-urlencoded")
        || m.contains("graphql")
}

/// The first `max` bytes of `s`, on a character boundary.
fn cut_bytes(s: &str, max: usize) -> &str {
    if s.len() <= max {
        return s;
    }
    let mut end = max;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

fn cookie_row(c: &Value) -> CookieRow {
    let value = c["value"].as_str().unwrap_or_default();
    let (value, truncated) = cut(value);
    let session = c["session"].as_bool() == Some(true);
    CookieRow {
        name: c["name"].as_str().unwrap_or_default().to_owned(),
        value,
        domain: c["domain"].as_str().unwrap_or_default().to_owned(),
        path: c["path"].as_str().unwrap_or("/").to_owned(),
        expires: if session {
            -1.
        } else {
            c["expires"].as_f64().unwrap_or(-1.)
        },
        http_only: c["httpOnly"].as_bool().unwrap_or(false),
        secure: c["secure"].as_bool().unwrap_or(false),
        same_site: c["sameSite"].as_str().map(str::to_owned),
        truncated,
    }
}

impl Browser {
    fn storage_id(origin: &str, local: bool) -> dom_storage::StorageId {
        dom_storage::StorageId {
            security_origin: Some(origin.to_owned()),
            storage_key: None,
            is_local_storage: local,
        }
    }

    fn storage_items(
        &self,
        tab: &Tab,
        origin: &str,
        local: bool,
    ) -> Result<Vec<StorageItem>, CommandError> {
        let r = self.call(
            tab,
            dom_storage::GetDOMStorageItemsParams {
                storage_id: Self::storage_id(origin, local),
            },
        )?;
        Ok(r.entries
            .into_iter()
            .filter_map(|e| {
                let mut e = e.into_iter();
                let key = e.next()?;
                let value = e.next().unwrap_or_default();
                Some(StorageItem::new(key, &value))
            })
            .collect())
    }

    fn cookies(&self, tab: &Tab, origin: &str) -> Result<Vec<Value>, CommandError> {
        let r = self.send(
            tab,
            "Network.getCookies",
            json!({"urls": [format!("{origin}/")]}),
        )?;
        Ok(r["cookies"].as_array().cloned().unwrap_or_default())
    }

    /// `storage`.
    pub(super) fn storage(
        &mut self,
        id: Option<&str>,
        kind: StorageKind,
        action: StorageAction,
        origin: Option<&str>,
    ) -> Result<StorageOutput, CommandError> {
        let tab = self.resolve(id)?;
        let origin = match origin {
            Some(o) => o.to_owned(),
            None => {
                let url = tab.state().url.clone();
                ParsedUrl::parse(&url)
                    .filter(|u| !u.host.is_empty())
                    .and_then(|u| u.origin())
                    .ok_or_else(|| {
                        invalid(format!(
                            "the tab's page ({url}) has no origin with a host; give `origin`"
                        ))
                    })?
            }
        };
        let _ = self.call(&tab, dom_storage::EnableParams {});
        let mut out = StorageOutput {
            tab: tab.id.clone(),
            origin: origin.clone(),
            action: action.as_str().to_owned(),
            ..StorageOutput::default()
        };
        let cookies = if kind.cookies() {
            Some(self.cookies(&tab, &origin)?)
        } else {
            None
        };
        let local = kind
            .local()
            .then(|| self.storage_items(&tab, &origin, true))
            .transpose()?;
        let session = kind
            .session()
            .then(|| self.storage_items(&tab, &origin, false))
            .transpose()?;
        let total = cookies.as_ref().map_or(0, Vec::len)
            + local.as_ref().map_or(0, Vec::len)
            + session.as_ref().map_or(0, Vec::len);
        out.total = total;
        match action {
            StorageAction::Get => {
                out.cookies = cookies.map(|c| c.iter().map(cookie_row).collect());
                out.local = local;
                out.session = session;
            }
            StorageAction::Clear => {
                if let Some(cookies) = &cookies {
                    let calls = cookies
                        .iter()
                        .map(|c| {
                            (
                                "Network.deleteCookies".to_owned(),
                                json!({"name": c["name"], "domain": c["domain"], "path": c["path"]}),
                            )
                        })
                        .collect();
                    for r in self.engine.send_many(
                        &tab.session,
                        calls,
                        crate::connection::DEFAULT_TIMEOUT,
                    ) {
                        r.map_err(|e| failed(format!("Network.deleteCookies: {e}")))?;
                    }
                }
                for (items, is_local) in [(&local, true), (&session, false)] {
                    if items.is_some() {
                        self.call(
                            &tab,
                            dom_storage::ClearParams {
                                storage_id: Self::storage_id(&origin, is_local),
                            },
                        )?;
                    }
                }
                out.cleared = Some(Cleared {
                    cookies: cookies.as_ref().map(Vec::len),
                    local: local.as_ref().map(Vec::len),
                    session: session.as_ref().map(Vec::len),
                });
                (self.log)(&format!(
                    "[{}] Cleared {total} cookies and storage items of {origin}",
                    tab.id
                ));
            }
        }
        Ok(out)
    }

    /// `network_body`.
    pub(super) fn network_body(
        &mut self,
        id: Option<&str>,
        request_id: &str,
        max_bytes: usize,
    ) -> Result<NetworkBodyOutput, CommandError> {
        let tab = self.resolve(id)?;
        let entry = tab.state().request(request_id).cloned().ok_or_else(|| {
            invalid(format!(
                "no request `{request_id}` in tab {}; request ids come from eludite.browser.network",
                tab.id
            ))
        })?;
        let url = entry.row.url.clone();
        let r = self
            .send(
                &tab,
                "Network.getResponseBody",
                json!({"requestId": request_id}),
            )
            .map_err(|e| {
                let why = entry
                    .row
                    .failed
                    .as_deref()
                    .map(|f| format!(" (the request failed: {f})"))
                    .unwrap_or_default();
                failed(format!(
                    "the browser has no body for request {request_id} ({url}){why}: it was never received, or was dropped from the browser's buffers. {e}"
                ))
            })?;
        let body = r["body"].as_str().unwrap_or_default();
        let base64 = r["base64Encoded"].as_bool().unwrap_or(false);
        let mime = entry.row.mime_type.clone().unwrap_or_default();
        let mut out = NetworkBodyOutput {
            tab: tab.id.clone(),
            request_id: request_id.to_owned(),
            url,
            status: entry.row.status,
            headers: entry
                .headers
                .clone()
                .into_iter()
                .collect::<BTreeMap<_, _>>(),
            mime_type: mime.clone(),
            ..NetworkBodyOutput::default()
        };
        if !base64 {
            out.size = body.len();
            out.truncated = body.len() > max_bytes;
            out.body = Some(cut_bytes(body, max_bytes).to_owned());
            return Ok(out);
        }
        let bytes = base64_decode(body)
            .ok_or_else(|| failed("the browser answered a body that is not base64"))?;
        out.size = bytes.len();
        out.truncated = bytes.len() > max_bytes;
        match std::str::from_utf8(&bytes) {
            Ok(text) if is_text_type(&mime) => {
                out.body = Some(cut_bytes(text, max_bytes).to_owned());
            }
            _ => out.body_base64 = Some(base64_encode(&bytes[..bytes.len().min(max_bytes)])),
        }
        Ok(out)
    }

    /// `open_external`.
    pub(super) fn open_external(
        &mut self,
        url: Option<&str>,
    ) -> Result<OpenExternalOutput, CommandError> {
        let url = match url {
            Some(u) => u.to_owned(),
            None => {
                let tab = self
                    .resolve(None)
                    .map_err(|_| invalid("there is no tab whose url to open; give `url`"))?;
                let url = tab.state().url.clone();
                if url.is_empty() {
                    return Err(invalid("the active tab has no url; give `url`"));
                }
                url
            }
        };
        let (program, args) = opener(&url, self.opener.as_deref());
        let command = std::iter::once(program.as_str())
            .chain(args.iter().map(String::as_str).filter(|a| !a.is_empty()))
            .collect::<Vec<_>>()
            .join(" ");
        let mut child = std::process::Command::new(&program)
            .no_console_window()
            .args(&args)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .map_err(|e| failed(format!("could not start `{program}`: {e}")))?;
        // Never wait for the system browser; reap the opener when it exits.
        let _ = std::thread::Builder::new()
            .name("browser-opener".into())
            .spawn(move || {
                let _ = child.wait();
            });
        (self.log)(&format!("Opened {url} in the system browser ({command})"));
        Ok(OpenExternalOutput { url, command })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_round_trips() {
        for s in ["", "f", "fo", "foo", "foob", "fooba", "foobar"] {
            let e = base64_encode(s.as_bytes());
            assert_eq!(base64_decode(&e).unwrap(), s.as_bytes(), "{s}");
        }
        assert_eq!(base64_encode(b"foobar"), "Zm9vYmFy");
        assert_eq!(base64_encode(&[0xff, 0x00]), "/wA=");
        assert!(base64_decode("a*b").is_none());
    }

    #[test]
    fn bodies_cut_on_character_boundaries_and_text_types() {
        assert_eq!(cut_bytes("h\u{e9}llo", 2), "h");
        assert_eq!(cut_bytes("abc", 10), "abc");
        assert!(is_text_type("application/json; charset=utf-8"));
        assert!(is_text_type("image/svg+xml"));
        assert!(!is_text_type("image/png"));
    }

    #[test]
    fn cookies_read_field_by_field() {
        let c = cookie_row(
            &json!({"name": "sid", "value": "x".repeat(1200), "domain": "127.0.0.1", "path": "/",
            "expires": -1, "session": true, "httpOnly": true, "secure": false, "sameSite": "Lax"}),
        );
        assert!(c.truncated && c.http_only);
        assert_eq!(c.expires, -1.);
        assert_eq!(c.same_site.as_deref(), Some("Lax"));
        let c = cookie_row(
            &json!({"name": "a", "value": "b", "domain": "d", "path": "/", "expires": 1.9e9}),
        );
        assert_eq!(c.expires, 1.9e9);
        assert_eq!(c.same_site, None);
    }

    #[test]
    fn the_opener_is_the_platforms_unless_set() {
        assert_eq!(
            opener("http://a/", Some("/bin/fake")),
            ("/bin/fake".to_owned(), vec!["http://a/".to_owned()])
        );
        let (p, a) = opener("http://localhost/", None);
        if std::env::var_os(OPENER_ENV).is_none() {
            let platform = if cfg!(target_os = "windows") {
                "cmd"
            } else if cfg!(target_os = "macos") {
                "open"
            } else {
                "xdg-open"
            };
            assert_eq!(p, platform);
        }
        assert_eq!(a.last().unwrap(), "http://localhost/");
    }
}
