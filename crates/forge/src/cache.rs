//! The cache on disk under the workspace's `.eludite/forge/`: the last answer of every read (so the windows open
//! offline with what they last saw) and, per url, the last response body with its `ETag` (so a conditional request
//! answered 304 is served from here). Nothing here is ever a secret: requests' headers are never written, only
//! response bodies and validators.

use std::path::{Path, PathBuf};

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::util::{hash_hex, now_secs};

/// The folder under a workspace.
pub const CACHE_DIR: &str = ".eludite/forge";

/// A cached answer and when the forge sent it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cached<T> {
    pub value: T,
    /// Seconds since the epoch.
    pub fetched_at: i64,
}

impl<T> Cached<T> {
    /// How old, in seconds.
    pub fn age(&self) -> u64 {
        (now_secs() - self.fetched_at).max(0) as u64
    }
}

#[derive(Serialize, Deserialize)]
struct AnswerFile<T> {
    key: String,
    fetched_at: i64,
    value: T,
}

/// A response body kept for conditional requests.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HttpEntry {
    pub url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub etag: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_modified: Option<String>,
    pub fetched_at: i64,
    /// Response headers worth keeping (paging links), never request headers.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub headers: Vec<(String, String)>,
    pub body: String,
}

/// The cache of one workspace (or a test's folder).
#[derive(Debug, Clone)]
pub struct Cache {
    root: PathBuf,
}

/// A path component made safe: letters, digits, `-`, `_`, `.`, `@` kept, the rest `_`; never `..`.
pub fn safe_component(s: &str) -> String {
    let mut out: String = s
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '@') {
                c
            } else {
                '_'
            }
        })
        .collect();
    if out.is_empty() || out.chars().all(|c| c == '.') {
        out = format!("_{out}");
    }
    out
}

impl Cache {
    /// The cache under `workspace/.eludite/forge`.
    pub fn for_workspace(workspace: &Path) -> Self {
        Self {
            root: workspace.join(CACHE_DIR),
        }
    }

    /// The cache rooted at `root` exactly.
    pub fn at(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    fn answer_path(&self, scope: &str, key: &str) -> PathBuf {
        let kind = key.split(':').next().unwrap_or("answer");
        self.root
            .join(scope)
            .join(safe_component(kind))
            .join(format!("{}.json", hash_hex(key)))
    }

    /// The last answer stored for `key` in `scope` (a repository's [`crate::Repository::cache_key`]).
    pub fn get<T: DeserializeOwned>(&self, scope: &str, key: &str) -> Option<Cached<T>> {
        let text = std::fs::read(self.answer_path(scope, key)).ok()?;
        let file: AnswerFile<T> = serde_json::from_slice(&text).ok()?;
        (file.key == key).then_some(Cached {
            value: file.value,
            fetched_at: file.fetched_at,
        })
    }

    /// Store `value` as the answer for `key`, fetched now.
    pub fn put<T: Serialize>(&self, scope: &str, key: &str, value: &T) {
        self.put_at(scope, key, value, now_secs());
    }

    /// Store with an explicit time (tests make a stale answer this way).
    pub fn put_at<T: Serialize>(&self, scope: &str, key: &str, value: &T, fetched_at: i64) {
        let file = AnswerFile {
            key: key.to_owned(),
            fetched_at,
            value,
        };
        if let Ok(bytes) = serde_json::to_vec(&file) {
            write_atomic(&self.answer_path(scope, key), &bytes);
        }
    }

    /// Forget one answer.
    pub fn remove(&self, scope: &str, key: &str) {
        let _ = std::fs::remove_file(self.answer_path(scope, key));
    }

    /// Forget every answer of one kind (`checks`) in `scope`.
    pub fn remove_kind(&self, scope: &str, kind: &str) {
        let _ = std::fs::remove_dir_all(self.root.join(scope).join(safe_component(kind)));
    }

    fn http_path(&self, url: &str) -> PathBuf {
        self.root
            .join("http")
            .join(format!("{}.json", hash_hex(url)))
    }

    /// The kept response for `url`.
    pub fn http_get(&self, url: &str) -> Option<HttpEntry> {
        let text = std::fs::read(self.http_path(url)).ok()?;
        let e: HttpEntry = serde_json::from_slice(&text).ok()?;
        (e.url == url).then_some(e)
    }

    /// Keep a response for `url`.
    pub fn http_put(&self, entry: &HttpEntry) {
        if let Ok(bytes) = serde_json::to_vec(entry) {
            write_atomic(&self.http_path(&entry.url), &bytes);
        }
    }

    /// Every file under the cache, for tests that look for what must never be there.
    pub fn files(&self) -> Vec<PathBuf> {
        let mut out = Vec::new();
        let mut stack = vec![self.root.clone()];
        while let Some(d) = stack.pop() {
            let Ok(rd) = std::fs::read_dir(&d) else {
                continue;
            };
            for e in rd.flatten() {
                let p = e.path();
                if p.is_dir() {
                    stack.push(p);
                } else {
                    out.push(p);
                }
            }
        }
        out
    }
}

fn write_atomic(path: &Path, bytes: &[u8]) {
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let tmp = path.with_extension(format!("tmp{}", std::process::id()));
    if std::fs::write(&tmp, bytes).is_ok() && std::fs::rename(&tmp, path).is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn answers_round_trip_and_keys_do_not_collide() {
        let dir = tempfile::tempdir().unwrap();
        let c = Cache::at(dir.path());
        c.put("github.com/o/r", "pulls:open", &vec![1, 2, 3]);
        let got: Cached<Vec<i32>> = c.get("github.com/o/r", "pulls:open").unwrap();
        assert_eq!(got.value, vec![1, 2, 3]);
        assert!(got.age() < 5);
        assert!(
            c.get::<Vec<i32>>("github.com/o/r", "pulls:closed")
                .is_none()
        );
        assert!(
            c.get::<Vec<i32>>("github.com/o/other", "pulls:open")
                .is_none()
        );
        c.put_at("s", "k", &"old", 1000);
        assert!(c.get::<String>("s", "k").unwrap().age() > 1_000_000);
    }

    #[test]
    fn components_are_made_safe() {
        assert_eq!(safe_component("../x"), ".._x");
        assert_eq!(safe_component(".."), "_..");
        assert_eq!(
            safe_component("gitlab.example.com:8443"),
            "gitlab.example.com_8443"
        );
        assert_eq!(safe_component("did:plc:abc"), "did_plc_abc");
    }
}
