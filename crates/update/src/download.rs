//! Downloading a release's files: `SHA256SUMS` first, then the archive streamed to a `.part` file beside its final
//! name, hashed as it arrives and kept only when its digest matches; the release list with a conditional request.

use std::collections::BTreeMap;
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};

use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::error::{Error, Result};
use crate::http::{Cancel, Progress, Transport};
use crate::release::{self, Release};

/// GitHub's REST API, unless a setup names another (a mirror, a test server).
pub const GITHUB_API: &str = "https://api.github.com";

/// The headers every request to the API carries.
pub const API_HEADERS: [(&str, &str); 2] = [
    ("Accept", "application/vnd.github+json"),
    ("X-GitHub-Api-Version", "2022-11-28"),
];

/// How many releases one page asks for: enough that a channel's newest is on the first page even when another
/// channel publishes often.
pub const RELEASES_PER_PAGE: u32 = 30;

/// Where releases are read from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Source {
    /// `https://api.github.com`, or a mirror.
    pub api: String,
    /// `sand-head/Eludite`.
    pub repository: String,
}

impl Source {
    pub fn github(repository: &str) -> Source {
        Source {
            api: GITHUB_API.to_owned(),
            repository: repository.to_owned(),
        }
    }

    /// The release list's url.
    pub fn releases_url(&self) -> String {
        format!(
            "{}/repos/{}/releases?per_page={RELEASES_PER_PAGE}",
            self.api.trim_end_matches('/'),
            self.repository
        )
    }
}

/// The release list as last read, with the `ETag` to ask again with.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ReleaseList {
    pub etag: Option<String>,
    pub releases: Vec<Release>,
}

/// Read the release list, conditionally when `cached` holds an `ETag`: a 304 answers with the cached list (and
/// does not count against GitHub's unauthenticated rate limit).
pub fn fetch_releases(
    transport: &dyn Transport,
    source: &Source,
    cached: Option<&ReleaseList>,
    cancel: &Cancel,
) -> Result<ReleaseList> {
    let url = source.releases_url();
    let mut headers: Vec<(&str, &str)> = API_HEADERS.to_vec();
    let etag = cached.and_then(|c| c.etag.clone());
    if let Some(etag) = etag.as_deref() {
        headers.push(("If-None-Match", etag));
    }
    let mut body = Vec::new();
    let answer = transport.get(&url, &headers, &mut body, &|_, _| {}, cancel)?;
    match answer.status {
        304 => Ok(cached.cloned().unwrap_or_default()),
        200 => {
            let json: Value = serde_json::from_slice(&body)
                .map_err(|e| Error::malformed(format!("the release list is not JSON ({e})")))?;
            let releases = Release::parse_list(&json).map_err(Error::malformed)?;
            Ok(ReleaseList {
                etag: answer.header("etag").map(str::to_owned),
                releases,
            })
        }
        status => Err(Error::Http {
            url,
            status,
            message: http_message(status, &body),
        }),
    }
}

/// A short reason from an API error body (`{"message": ...}`), or the status's usual meaning.
fn http_message(status: u16, body: &[u8]) -> String {
    if let Ok(v) = serde_json::from_slice::<Value>(body)
        && let Some(m) = v["message"].as_str()
    {
        return m.to_owned();
    }
    match status {
        403 | 429 => "rate limited; try again later".into(),
        404 => "not found".into(),
        s if s >= 500 => "the server failed".into(),
        _ => String::from_utf8_lossy(&body[..body.len().min(200)]).into_owned(),
    }
}

/// Read and parse a release's `SHA256SUMS`.
pub fn fetch_sums(
    transport: &dyn Transport,
    url: &str,
    cancel: &Cancel,
) -> Result<BTreeMap<String, String>> {
    let mut body = Vec::new();
    let answer = transport.get(url, &[], &mut body, &|_, _| {}, cancel)?;
    if answer.status != 200 {
        return Err(Error::Http {
            url: url.to_owned(),
            status: answer.status,
            message: http_message(answer.status, &body),
        });
    }
    release::parse_sums(&String::from_utf8_lossy(&body)).map_err(Error::malformed)
}

/// A writer that hashes what passes through it.
struct Hashing<W: Write> {
    inner: W,
    hash: Sha256,
}

impl<W: Write> Write for Hashing<W> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let n = self.inner.write(buf)?;
        self.hash.update(&buf[..n]);
        Ok(n)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

/// Download `url` to `dest` (through `dest` + `.part`), verifying its SHA-256 against `expected` (lower-case hex).
/// A mismatch deletes the file and fails with [`Error::Verification`]; a cancellation deletes the partial file. An
/// existing `dest` whose digest already matches is kept and not downloaded again.
pub fn download_verified(
    transport: &dyn Transport,
    url: &str,
    dest: &Path,
    expected: &str,
    progress: Progress<'_>,
    cancel: &Cancel,
) -> Result<PathBuf> {
    if dest.is_file() && file_sha256(dest)? == expected {
        return Ok(dest.to_owned());
    }
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let part = part_path(dest);
    let result = (|| -> Result<()> {
        let file = File::create(&part)?;
        let mut sink = Hashing {
            inner: BufWriter::new(file),
            hash: Sha256::new(),
        };
        let answer = transport.get(url, &[], &mut sink, progress, cancel)?;
        if answer.status != 200 {
            return Err(Error::Http {
                url: url.to_owned(),
                status: answer.status,
                message: http_message(answer.status, b""),
            });
        }
        sink.flush()?;
        let Hashing { inner, hash } = sink;
        drop(inner);
        let actual = hex(&hash.finalize());
        if actual != expected {
            return Err(Error::Verification {
                name: dest
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default(),
                expected: expected.to_owned(),
                actual,
            });
        }
        if dest.exists() {
            std::fs::remove_file(dest)?;
        }
        std::fs::rename(&part, dest)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&part);
    }
    result.map(|()| dest.to_owned())
}

/// `<name>.part` beside `dest`.
pub fn part_path(dest: &Path) -> PathBuf {
    let mut name = dest.file_name().map(|n| n.to_owned()).unwrap_or_default();
    name.push(".part");
    dest.with_file_name(name)
}

/// The lower-case hex SHA-256 of a file.
pub fn file_sha256(path: &Path) -> std::io::Result<String> {
    let mut file = File::open(path)?;
    let mut hash = Sha256::new();
    std::io::copy(&mut file, &mut hash)?;
    Ok(hex(&hash.finalize()))
}

/// Lower-case hex of bytes.
pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls_and_hex() {
        let s = Source::github("sand-head/Eludite");
        assert_eq!(
            s.releases_url(),
            "https://api.github.com/repos/sand-head/Eludite/releases?per_page=30"
        );
        assert_eq!(hex(&[0, 255, 16]), "00ff10");
        assert_eq!(
            part_path(Path::new("/a/b.tar.gz")),
            PathBuf::from("/a/b.tar.gz.part")
        );
    }

    #[test]
    fn http_messages_read_the_api_body() {
        assert_eq!(
            http_message(403, br#"{"message":"API rate limit exceeded"}"#),
            "API rate limit exceeded"
        );
        assert_eq!(http_message(403, b"nope"), "rate limited; try again later");
        assert_eq!(http_message(404, b""), "not found");
        assert_eq!(http_message(502, b""), "the server failed");
    }
}
