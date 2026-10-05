//! For the tests of this crate and the shell's (feature `test-support`): a loopback HTTP/1.1 server serving files
//! from a folder, with `ETag` and `If-None-Match` on the release list, a request log and a switch that stalls an
//! archive so a cancellation can land mid-download; releases published into that folder; an install folder.

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::build::{Build, BuildId, Channel};
use crate::extract::test_support::tar_gz;
use serde_json::{Value, json};

/// The requests seen: the target and the headers.
pub type Requests = Arc<Mutex<Vec<(String, BTreeMap<String, String>)>>>;

pub struct Server {
    pub dir: PathBuf,
    port: u16,
    stop: Arc<AtomicBool>,
    pub requests: Requests,
    /// Milliseconds to sleep per 64 KiB chunk of a file named with `slow` (so a cancel lands mid-download).
    pub slow_ms: Arc<AtomicU64>,
    pub status_override: Arc<Mutex<BTreeMap<String, u16>>>,
}

impl Server {
    pub fn serve(dir: &Path) -> Server {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let stop = Arc::new(AtomicBool::new(false));
        let requests: Requests = Arc::default();
        let slow_ms = Arc::new(AtomicU64::new(0));
        let status_override: Arc<Mutex<BTreeMap<String, u16>>> = Arc::default();
        {
            let (dir, stop, requests, slow_ms, status_override) = (
                dir.to_owned(),
                stop.clone(),
                requests.clone(),
                slow_ms.clone(),
                status_override.clone(),
            );
            std::thread::spawn(move || {
                for conn in listener.incoming() {
                    if stop.load(Ordering::SeqCst) {
                        break;
                    }
                    let Ok(conn) = conn else { continue };
                    let (dir, requests, slow_ms, status_override) = (
                        dir.clone(),
                        requests.clone(),
                        slow_ms.clone(),
                        status_override.clone(),
                    );
                    std::thread::spawn(move || {
                        let _ = handle(conn, &dir, &requests, &slow_ms, &status_override);
                    });
                }
            });
        }
        Server {
            dir: dir.to_owned(),
            port,
            stop,
            requests,
            slow_ms,
            status_override,
        }
    }

    pub fn base(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }

    pub fn paths(&self) -> Vec<String> {
        self.requests
            .lock()
            .unwrap()
            .iter()
            .map(|(p, _)| p.clone())
            .collect()
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        let _ = TcpStream::connect(("127.0.0.1", self.port));
    }
}

fn handle(
    conn: TcpStream,
    dir: &Path,
    requests: &Mutex<Vec<(String, BTreeMap<String, String>)>>,
    slow_ms: &AtomicU64,
    status_override: &Mutex<BTreeMap<String, u16>>,
) -> std::io::Result<()> {
    conn.set_read_timeout(Some(Duration::from_secs(5)))?;
    let mut reader = BufReader::new(conn.try_clone()?);
    let mut out = conn;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line)? == 0 {
            return Ok(());
        }
        let mut parts = line.split_whitespace();
        let (Some(method), Some(target)) = (parts.next(), parts.next()) else {
            return Ok(());
        };
        let mut headers = BTreeMap::new();
        loop {
            let mut h = String::new();
            reader.read_line(&mut h)?;
            let h = h.trim_end();
            if h.is_empty() {
                break;
            }
            if let Some((k, v)) = h.split_once(':') {
                headers.insert(k.trim().to_ascii_lowercase(), v.trim().to_owned());
            }
        }
        let path = target.split('?').next().unwrap_or(target).to_owned();
        requests
            .lock()
            .unwrap()
            .push((target.to_owned(), headers.clone()));
        let file = dir.join(path.trim_start_matches('/'));
        let overridden = status_override.lock().unwrap().get(&path).copied();
        let (status, body, extra): (u16, Vec<u8>, Vec<String>) = match (method, overridden) {
            (_, Some(code)) => (
                code,
                format!("{{\"message\":\"forced {code}\"}}").into_bytes(),
                vec![],
            ),
            ("GET", None) if file.is_file() => {
                let body = std::fs::read(&file)?;
                let etag = format!(
                    "\"{}\"",
                    crate::download::hex(&sha2_256(&body))[..16].to_owned()
                );
                if headers.get("if-none-match").map(String::as_str) == Some(etag.as_str()) {
                    (304, Vec::new(), vec![format!("ETag: {etag}")])
                } else {
                    (200, body, vec![format!("ETag: {etag}")])
                }
            }
            _ => (404, b"{\"message\":\"Not Found\"}".to_vec(), vec![]),
        };
        let reason = match status {
            200 => "OK",
            304 => "Not Modified",
            404 => "Not Found",
            _ => "Error",
        };
        write!(
            out,
            "HTTP/1.1 {status} {reason}\r\nContent-Length: {}\r\nConnection: keep-alive\r\n",
            body.len()
        )?;
        for h in extra {
            write!(out, "{h}\r\n")?;
        }
        write!(out, "\r\n")?;
        let slow = slow_ms.load(Ordering::SeqCst);
        if slow > 0 && path.contains("slow") {
            for chunk in body.chunks(64 * 1024) {
                out.write_all(chunk)?;
                out.flush()?;
                std::thread::sleep(Duration::from_millis(slow));
            }
        } else {
            out.write_all(&body)?;
        }
        out.flush()?;
    }
}

fn sha2_256(bytes: &[u8]) -> Vec<u8> {
    use sha2::Digest as _;
    sha2::Sha256::digest(bytes).to_vec()
}

/// A release's files in `dir` under `releases/<tag>/`: the archive for `os`-`arch` carrying `build.json` for the
/// tag's build and the given `files`, and `SHA256SUMS` listing it. Returns the release's JSON for the list.
pub fn publish(
    dir: &Path,
    base: &str,
    tag: &str,
    os: &str,
    arch: &str,
    files: &[(&str, &[u8], u32)],
) -> Value {
    let build = Channel::Unstable.build_of_tag(tag).unwrap();
    let folder = dir.join("releases").join(tag);
    std::fs::create_dir_all(&folder).unwrap();
    let name = format!("eludite-0.1.0-{os}-{arch}.tar.gz");
    let build_json = serde_json::to_vec_pretty(&Build {
        version: "0.1.0".into(),
        channel: Channel::Unstable,
        build,
        commit: Some("0123456789abcdef0123456789abcdef01234567".into()),
        os: os.into(),
        arch: arch.into(),
        published: Some("2026-10-05T12:00:00Z".into()),
    })
    .unwrap();
    let mut all: Vec<(&str, &[u8], u32)> = files.to_vec();
    all.push(("build.json", &build_json, 0o644));
    tar_gz(
        &folder.join(&name),
        &format!("eludite-0.1.0-{os}-{arch}"),
        &all,
    );
    let digest = crate::download::file_sha256(&folder.join(&name)).unwrap();
    std::fs::write(folder.join("SHA256SUMS"), format!("{digest}  {name}\n")).unwrap();
    let size = std::fs::metadata(folder.join(&name)).unwrap().len();
    json!({
        "tag_name": tag, "name": tag, "prerelease": true, "draft": false,
        "published_at": "2026-10-05T12:00:00Z", "html_url": format!("{base}/html/{tag}"),
        "assets": [
            {"name": name, "size": size, "browser_download_url": format!("{base}/releases/{tag}/{name}")},
            {"name": "SHA256SUMS", "size": 100, "browser_download_url": format!("{base}/releases/{tag}/SHA256SUMS")}
        ]
    })
}

/// Write the release list the API path answers with.
pub fn list(dir: &Path, repository: &str, releases: &[Value]) {
    let p = dir.join("repos").join(repository).join("releases");
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(
        &p,
        serde_json::to_vec(&Value::Array(releases.to_vec())).unwrap(),
    )
    .unwrap();
}

/// An install folder with `build.json` for `tag` and an `eludite` file.
pub fn install(dir: &Path, tag: &str, os: &str, arch: &str) -> PathBuf {
    let install = dir.join("install");
    std::fs::create_dir_all(&install).unwrap();
    std::fs::write(install.join("eludite"), b"old binary").unwrap();
    Build {
        version: "0.1.0".into(),
        channel: Channel::Unstable,
        build: BuildId::parse(Channel::Unstable.build_of_tag(tag).unwrap().as_str()).unwrap(),
        commit: None,
        os: os.into(),
        arch: arch.into(),
        published: None,
    }
    .write(&install)
    .unwrap();
    install
}

/// `n` bytes that do not compress (a linear congruential generator), so an archive's size is what it seems.
pub fn noise(n: usize) -> Vec<u8> {
    let mut x: u64 = 0x9E37_79B9_7F4A_7C15;
    (0..n)
        .map(|_| {
            x = x
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            (x >> 33) as u8
        })
        .collect()
}
