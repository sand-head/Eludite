//! Test servers for git over http(s) (brief 0045), on loopback, with no network and no new dependency:
//!
//! - **Smart HTTP.** A small HTTP/1.1 listener on `std::net` that demands basic authentication (so the credential
//!   callback is exercised) and runs `git http-backend` as a CGI program for each authorized request. It also
//!   answers requests in proxy form (`GET http://host/path`), so a test can point `http.proxy` at it.
//! - **TLS.** `openssl s_server` terminates TLS with a self-signed certificate made by `openssl req` in a temporary
//!   folder, and hands the decrypted bytes to the same request loop over its standard input and output (`-quiet`:
//!   nothing else is printed, and its single-letter commands are off). One connection at a time, kept alive, as
//!   libgit2 uses it. Unix only: `s_server` reads a Windows pipe as a console.
//!
//! The servers start `git` and `openssl` processes; the product never does. Each `start` returns `None` (the test
//! skips) when the program is not on `PATH`.
//!
//! Shared by `crates/git/tests/remote.rs` and the shell's `git_tests.rs` (included there by path).

#![allow(dead_code)]

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// One request the server answered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Logged {
    pub method: String,
    /// As sent: a path, or a whole url when the client spoke to a proxy.
    pub target: String,
    /// The user of a valid `Authorization` header.
    pub user: Option<String>,
    pub status: u16,
}

struct Config {
    root: PathBuf,
    user: String,
    password: String,
    log: Mutex<Vec<Logged>>,
}

/// `git http-backend` behind basic authentication, over http or https.
pub struct GitHttp {
    config: Arc<Config>,
    pub port: u16,
    tls: bool,
    stop: Arc<AtomicBool>,
    child: Option<Child>,
    _cert_dir: Option<tempfile::TempDir>,
}

/// Whether `program` runs (`program --version` or `program version`).
pub fn on_path(program: &str, arg: &str) -> bool {
    Command::new(program)
        .arg(arg)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

fn base64(bytes: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for c in bytes.chunks(3) {
        let n = (u32::from(c[0]) << 16)
            | (u32::from(*c.get(1).unwrap_or(&0)) << 8)
            | u32::from(*c.get(2).unwrap_or(&0));
        for i in 0..4 {
            if i <= c.len() {
                out.push(T[(n >> (18 - 6 * i)) as usize & 63] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

impl GitHttp {
    /// Serve the repositories under `root` over plain http, for `user` and `password`.
    pub fn start(root: &Path, user: &str, password: &str) -> Option<GitHttp> {
        if !on_path("git", "version") {
            eprintln!("skipped: no `git` on PATH for the test server");
            return None;
        }
        let config = Arc::new(Config {
            root: root.to_path_buf(),
            user: user.into(),
            password: password.into(),
            log: Mutex::new(Vec::new()),
        });
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let stop = Arc::new(AtomicBool::new(false));
        {
            let (config, stop) = (config.clone(), stop.clone());
            std::thread::spawn(move || {
                for stream in listener.incoming() {
                    if stop.load(Ordering::SeqCst) {
                        break;
                    }
                    let Ok(stream) = stream else { continue };
                    let config = config.clone();
                    std::thread::spawn(move || {
                        let reader = BufReader::new(stream.try_clone().unwrap());
                        let _ = serve(reader, stream, &config);
                    });
                }
            });
        }
        Some(GitHttp {
            config,
            port,
            tls: false,
            stop,
            child: None,
            _cert_dir: None,
        })
    }

    /// Serve over https: `openssl s_server` with a fresh self-signed certificate in front of the request loop.
    pub fn start_tls(root: &Path, user: &str, password: &str) -> Option<GitHttp> {
        if cfg!(windows) {
            eprintln!("skipped: openssl s_server cannot read a pipe on Windows");
            return None;
        }
        if !on_path("git", "version") || !on_path("openssl", "version") {
            eprintln!("skipped: the https test server needs `git` and `openssl` on PATH");
            return None;
        }
        let dir = tempfile::tempdir().unwrap();
        let (cert, key) = (dir.path().join("cert.pem"), dir.path().join("key.pem"));
        let made = Command::new("openssl")
            .args([
                "req", "-x509", "-newkey", "rsa:2048", "-nodes", "-days", "2",
            ])
            .args(["-subj", "/CN=localhost", "-keyout"])
            .arg(&key)
            .arg("-out")
            .arg(&cert)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .unwrap();
        assert!(made.success(), "openssl req failed");
        let port = TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let mut child = Command::new("openssl")
            .args(["s_server", "-quiet", "-accept"])
            .arg(format!("127.0.0.1:{port}"))
            .arg("-cert")
            .arg(&cert)
            .arg("-key")
            .arg(&key)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let config = Arc::new(Config {
            root: root.to_path_buf(),
            user: user.into(),
            password: password.into(),
            log: Mutex::new(Vec::new()),
        });
        {
            let (stdout, stdin) = (child.stdout.take().unwrap(), child.stdin.take().unwrap());
            let config = config.clone();
            std::thread::spawn(move || {
                let _ = serve(BufReader::new(stdout), stdin, &config);
            });
        }
        // Listening once a connection is taken (s_server drops this probe as a failed handshake).
        let deadline = Instant::now() + eludite_test_support::hang_bound(Duration::from_secs(10));
        while TcpStream::connect(("127.0.0.1", port)).is_err() {
            assert!(Instant::now() < deadline, "openssl s_server did not listen");
            std::thread::sleep(Duration::from_millis(20));
        }
        Some(GitHttp {
            config,
            port,
            tls: true,
            stop: Arc::new(AtomicBool::new(false)),
            child: Some(child),
            _cert_dir: Some(dir),
        })
    }

    /// The url of repository `name` (a folder under the root).
    pub fn url(&self, name: &str) -> String {
        format!(
            "{}://127.0.0.1:{}/{name}",
            if self.tls { "https" } else { "http" },
            self.port
        )
    }

    /// `127.0.0.1:<port>`, the host credential messages name.
    pub fn host(&self) -> String {
        format!("127.0.0.1:{}", self.port)
    }

    /// The requests answered so far.
    pub fn requests(&self) -> Vec<Logged> {
        self.config.log.lock().unwrap().clone()
    }
}

impl Drop for GitHttp {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(mut c) = self.child.take() {
            let _ = c.kill();
            let _ = c.wait();
        } else {
            let _ = TcpStream::connect(("127.0.0.1", self.port));
        }
    }
}

/// Answer HTTP/1.1 requests from `r` on `w` until the stream ends.
fn serve(mut r: impl BufRead, mut w: impl Write, config: &Config) -> std::io::Result<()> {
    loop {
        let mut line = String::new();
        if r.read_line(&mut line)? == 0 {
            return Ok(());
        }
        let line = line.trim_end();
        if line.is_empty() {
            continue;
        }
        let mut parts = line.split(' ');
        let method = parts.next().unwrap_or_default().to_owned();
        let target = parts.next().unwrap_or_default().to_owned();
        let mut headers: HashMap<String, String> = HashMap::new();
        loop {
            let mut h = String::new();
            if r.read_line(&mut h)? == 0 {
                return Ok(());
            }
            let h = h.trim_end();
            if h.is_empty() {
                break;
            }
            if let Some((k, v)) = h.split_once(':') {
                headers.insert(k.trim().to_ascii_lowercase(), v.trim().to_owned());
            }
        }
        let body = read_body(&mut r, &headers)?;
        let expected = format!(
            "Basic {}",
            base64(format!("{}:{}", config.user, config.password).as_bytes())
        );
        let authorized = headers.get("authorization") == Some(&expected);
        let path = match target.split_once("://") {
            // Proxy form: `GET http://host/path`.
            Some((_, rest)) => rest.find('/').map_or("/", |i| &rest[i..]).to_owned(),
            None => target.clone(),
        };
        let (status, response_headers, response_body) = if authorized {
            cgi(config, &method, &path, &headers, &body)
        } else {
            (
                401,
                vec![
                    (
                        "WWW-Authenticate".to_owned(),
                        "Basic realm=\"eludite-test\"".to_owned(),
                    ),
                    ("Content-Type".to_owned(), "text/plain".to_owned()),
                ],
                b"authentication required\n".to_vec(),
            )
        };
        config.log.lock().unwrap().push(Logged {
            method,
            target,
            user: authorized.then(|| config.user.clone()),
            status,
        });
        let mut head = format!("HTTP/1.1 {status} {}\r\n", reason(status));
        for (k, v) in &response_headers {
            head.push_str(&format!("{k}: {v}\r\n"));
        }
        head.push_str(&format!(
            "Content-Length: {}\r\nConnection: keep-alive\r\n\r\n",
            response_body.len()
        ));
        w.write_all(head.as_bytes())?;
        w.write_all(&response_body)?;
        w.flush()?;
    }
}

fn reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        _ => "Status",
    }
}

fn read_body(r: &mut impl BufRead, headers: &HashMap<String, String>) -> std::io::Result<Vec<u8>> {
    let mut body = Vec::new();
    if headers
        .get("transfer-encoding")
        .is_some_and(|t| t.eq_ignore_ascii_case("chunked"))
    {
        loop {
            let mut size = String::new();
            r.read_line(&mut size)?;
            let n = usize::from_str_radix(size.trim().split(';').next().unwrap_or("0"), 16)
                .unwrap_or(0);
            if n == 0 {
                // Trailers, then the empty line.
                loop {
                    let mut t = String::new();
                    if r.read_line(&mut t)? == 0 || t.trim_end().is_empty() {
                        break;
                    }
                }
                break;
            }
            let start = body.len();
            body.resize(start + n, 0);
            r.read_exact(&mut body[start..])?;
            let mut crlf = String::new();
            r.read_line(&mut crlf)?;
        }
    } else if let Some(n) = headers
        .get("content-length")
        .and_then(|v| v.parse::<usize>().ok())
    {
        body.resize(n, 0);
        r.read_exact(&mut body)?;
    }
    Ok(body)
}

type Response = (u16, Vec<(String, String)>, Vec<u8>);

/// Run `git http-backend` for one request.
fn cgi(
    config: &Config,
    method: &str,
    path: &str,
    headers: &HashMap<String, String>,
    body: &[u8],
) -> Response {
    let (path_info, query) = path.split_once('?').unwrap_or((path, ""));
    let mut cmd = Command::new("git");
    cmd.arg("http-backend")
        .env("GIT_PROJECT_ROOT", &config.root)
        .env("GIT_HTTP_EXPORT_ALL", "1")
        // The server's git reads no user or system config of this machine.
        .env("HOME", &config.root)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("PATH_INFO", path_info)
        .env("QUERY_STRING", query)
        .env("REQUEST_METHOD", method)
        .env("REMOTE_USER", &config.user)
        .env("REMOTE_ADDR", "127.0.0.1")
        .env("CONTENT_LENGTH", body.len().to_string())
        .env(
            "CONTENT_TYPE",
            headers.get("content-type").cloned().unwrap_or_default(),
        )
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    if let Some(p) = headers.get("git-protocol") {
        cmd.env("GIT_PROTOCOL", p);
    }
    if let Some(e) = headers.get("content-encoding") {
        cmd.env("HTTP_CONTENT_ENCODING", e);
    }
    let mut child = cmd.spawn().expect("git http-backend");
    let mut stdin = child.stdin.take().unwrap();
    let input = body.to_vec();
    let writer = std::thread::spawn(move || {
        let _ = stdin.write_all(&input);
    });
    let out = child.wait_with_output().unwrap();
    let _ = writer.join();
    let raw = out.stdout;
    let split = raw
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .map(|i| (i, 4))
        .or_else(|| raw.windows(2).position(|w| w == b"\n\n").map(|i| (i, 2)));
    let Some((at, sep)) = split else {
        return (500, Vec::new(), raw);
    };
    let mut status = 200;
    let mut headers = Vec::new();
    for line in String::from_utf8_lossy(&raw[..at]).lines() {
        if let Some((k, v)) = line.split_once(':') {
            if k.eq_ignore_ascii_case("status") {
                status = v
                    .trim()
                    .split(' ')
                    .next()
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(500);
            } else {
                headers.push((k.trim().to_owned(), v.trim().to_owned()));
            }
        }
    }
    (status, headers, raw[at + sep..].to_vec())
}

/// An https proxy on loopback: answers `CONNECT host:port` with 200 and relays the tunnel to `target_port` on
/// loopback (whatever host was asked for), logging each CONNECT target.
pub struct ConnectProxy {
    pub port: u16,
    log: Arc<Mutex<Vec<String>>>,
    stop: Arc<AtomicBool>,
}

impl ConnectProxy {
    pub fn start(target_port: u16) -> ConnectProxy {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let log = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));
        {
            let (log, stop) = (log.clone(), stop.clone());
            std::thread::spawn(move || {
                for stream in listener.incoming() {
                    if stop.load(Ordering::SeqCst) {
                        break;
                    }
                    let Ok(client) = stream else { continue };
                    let log = log.clone();
                    std::thread::spawn(move || {
                        let _ = tunnel(client, target_port, &log);
                    });
                }
            });
        }
        ConnectProxy { port, log, stop }
    }

    /// The CONNECT targets asked for so far (`host:port`).
    pub fn connects(&self) -> Vec<String> {
        self.log.lock().unwrap().clone()
    }
}

impl Drop for ConnectProxy {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        let _ = TcpStream::connect(("127.0.0.1", self.port));
    }
}

fn tunnel(client: TcpStream, target_port: u16, log: &Mutex<Vec<String>>) -> std::io::Result<()> {
    let mut reader = BufReader::new(client.try_clone()?);
    let mut line = String::new();
    reader.read_line(&mut line)?;
    loop {
        let mut h = String::new();
        if reader.read_line(&mut h)? == 0 || h.trim_end().is_empty() {
            break;
        }
    }
    let mut parts = line.split_whitespace();
    let (method, target) = (
        parts.next().unwrap_or_default(),
        parts.next().unwrap_or_default(),
    );
    let mut out = client.try_clone()?;
    if method != "CONNECT" {
        out.write_all(b"HTTP/1.1 405 Method Not Allowed\r\nContent-Length: 0\r\n\r\n")?;
        return Ok(());
    }
    log.lock().unwrap().push(target.to_owned());
    let server = TcpStream::connect(("127.0.0.1", target_port))?;
    out.write_all(b"HTTP/1.1 200 Connection established\r\n\r\n")?;
    let mut to_server = server.try_clone()?;
    let mut from_server = server;
    let up = std::thread::spawn(move || {
        // Anything the reader buffered past the CONNECT head belongs to the tunnel.
        let buffered = reader.buffer().to_vec();
        let _ = to_server.write_all(&buffered);
        let mut rest = reader.into_inner();
        let _ = std::io::copy(&mut rest, &mut to_server);
        let _ = to_server.shutdown(std::net::Shutdown::Write);
    });
    let _ = std::io::copy(&mut from_server, &mut out);
    let _ = out.shutdown(std::net::Shutdown::Write);
    let _ = up.join();
    Ok(())
}

#[test]
fn base64_encodes_as_rfc_4648() {
    assert_eq!(base64(b"u:p"), "dTpw");
    assert_eq!(base64(b"user:pass"), "dXNlcjpwYXNz");
    assert_eq!(base64(b"ab"), "YWI=");
    assert_eq!(base64(b"a"), "YQ==");
}
