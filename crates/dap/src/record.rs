//! The DAP recorder (brief 0033): a [`Connection`] wrapper that logs every message in both directions, with a
//! monotonic timestamp, to a `.dap.json` file, scrubbed of what belongs to the machine that recorded it.
//!
//! The format ([`Recording`]) is one JSON object: `adapter`, `version`, `recorded_at`, `platform`, `description`,
//! `ended` and `messages: [{ t_ms, dir: "client" | "adapter", message }]`, written with one message per line so a
//! re-recording diffs line by line. Before it is written:
//!
//! - every absolute path under a scrub root (the repository, the scenario's temporary directory, the Mono executable)
//!   becomes its placeholder (`${ROOT}`, `${TMP}`, `${MONO}`), the rest of the path with `/` separators;
//! - every process id the session named (the `process` event's `systemProcessId`, `attach`'s `processId` or `pid`,
//!   ids the caller adds) becomes `${PID}`, as a number anywhere and as a word in text (ids of 1000 and over);
//! - timestamps in text (`2026-10-03T12:34:56.789Z`, `2026-10-03 12:34:56`) become `${TIME}`;
//! - a `variables` answer keeps its first [`MAX_RECORDED_VARIABLES`] rows and says `"truncated_by_recorder": true`;
//! - the run's own tokens the caller adds ([`RecordHandle::add_token`]: a page's origin, a browser's DevTools port and
//!   target id; brief 0038) become their placeholders: a token of digits as a number, any other in text.
//!
//! The connections of one adapter server (vscode-js-debug's: one per debugging session, brief 0038) are recorded with
//! one counter ([`record_ordered`]): each message carries its `order` across them, and the replayer holds an adapter
//! message until the client's messages of every connection that came before it have arrived
//! (`replay::ReplayGroup`).
//!
//! A test's action outside DAP that the adapter's next messages follow (a click in a page, brief 0038) is a mark,
//! [`RecordHandle::mark`]: a client-side entry `{"type": "mark", "label": ...}` that the replayer holds the adapter's
//! later messages behind until the test passes the same mark.
//!
//! The replayer (`replay`, feature `replay`) substitutes the test's own values back ([`Substitution`]), and scrubs
//! the client's requests with the same rules before it matches them ([`Scrubber`]). [`compare`] is the re-record
//! check: two recordings of one scenario agree when their messages agree, timing aside.
//!
//! Recordings are only ever produced by this recorder from a real adapter; they are never edited by hand.

use std::collections::BTreeMap;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::Connection;

/// The rows a recorded `variables` answer keeps.
pub const MAX_RECORDED_VARIABLES: usize = 50;
/// The largest recording the corpus takes (brief 0033's budget).
pub const MAX_RECORDING_BYTES: usize = 2 * 1024 * 1024;
/// Process ids below this are not replaced in text (too likely to be some other number).
const MIN_TEXT_PID: i64 = 1000;
/// The `type` of a mark: a test's action outside DAP, recorded where it happened ([`RecordHandle::mark`]).
pub const MARK: &str = "mark";

/// Who sent a message.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Dir {
    Client,
    Adapter,
}

/// One message of a recording.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RecordedMessage {
    /// Milliseconds since the connection opened.
    pub t_ms: u64,
    pub dir: Dir,
    /// The DAP message as sent (scrubbed).
    pub message: Value,
    /// Its place among the messages of every connection recorded with the same counter ([`record_ordered`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub order: Option<u64>,
}

/// Which side closed the connection first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Ended {
    /// The adapter closed its output (it exited).
    Adapter,
    /// The client hung up first.
    Client,
}

/// A recorded DAP session (`<adapter>/<scenario>.dap.json`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Recording {
    /// `mono`, `lldb`, `netcoredbg`.
    pub adapter: String,
    /// The adapter's version (`eludite-dbg-mono under mono 6.8.0.105`, `lldb-dap 18.1.3`).
    pub version: String,
    /// When it was recorded (UTC, ISO 8601).
    pub recorded_at: String,
    /// `linux-x86_64`.
    pub platform: String,
    /// The connection's description when recorded (`eludite-dbg-mono.exe (stdio)`), scrubbed.
    #[serde(default)]
    pub description: String,
    /// Which side ended the session; `None` when the recording was written while it was still open.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended: Option<Ended>,
    pub messages: Vec<RecordedMessage>,
}

impl Recording {
    /// Read a `.dap.json` file.
    pub fn read(path: &Path) -> io::Result<Self> {
        let text = std::fs::read_to_string(path)?;
        serde_json::from_str(&text).map_err(|e| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("{}: {e}", path.display()),
            )
        })
    }

    /// The file's text: the header fields one per line, then one message per line.
    pub fn to_text(&self) -> String {
        let mut out = String::from("{\n");
        let field = |out: &mut String, k: &str, v: Value| {
            out.push_str(&format!("  {}: {},\n", json!(k), v));
        };
        field(&mut out, "adapter", json!(self.adapter));
        field(&mut out, "version", json!(self.version));
        field(&mut out, "recorded_at", json!(self.recorded_at));
        field(&mut out, "platform", json!(self.platform));
        field(&mut out, "description", json!(self.description));
        if let Some(e) = self.ended {
            field(&mut out, "ended", json!(e));
        }
        out.push_str("  \"messages\": [\n");
        for (i, m) in self.messages.iter().enumerate() {
            let line = serde_json::to_string(m).expect("a message serializes");
            out.push_str("    ");
            out.push_str(&line);
            out.push_str(if i + 1 < self.messages.len() {
                ",\n"
            } else {
                "\n"
            });
        }
        out.push_str("  ]\n}\n");
        out
    }

    /// Write the file, creating its folder.
    pub fn write(&self, path: &Path) -> io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(path, self.to_text())
    }

    /// The client's requests, in order: (command, arguments).
    pub fn requests(&self) -> Vec<(String, Value)> {
        self.messages
            .iter()
            .filter(|m| m.dir == Dir::Client && m.message["type"] == "request")
            .map(|m| {
                (
                    m.message["command"].as_str().unwrap_or_default().to_owned(),
                    m.message.get("arguments").cloned().unwrap_or(Value::Null),
                )
            })
            .collect()
    }
}

/// `linux-x86_64`, `windows-x86_64`, `macos-aarch64`.
pub fn platform() -> String {
    format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH)
}

/// Now, as `2026-10-03T12:34:56Z`.
pub fn utc_now() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    // Howard Hinnant's civil_from_days.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

/// One scrub root: a placeholder and the spellings of its path on this machine.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Root {
    placeholder: String,
    variants: Vec<String>,
}

/// Replaces machine paths, process ids and timestamps with placeholders (the recorder's rules, also applied to the
/// client's requests by the replayer before matching).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Scrubber {
    roots: Vec<Root>,
    pids: Vec<i64>,
    /// (placeholder, text): the run's own values (brief 0038).
    tokens: Vec<(String, String)>,
}

/// Whether a token is a number (replaced as a JSON number, never inside text).
fn numeric(text: &str) -> bool {
    !text.is_empty() && text.bytes().all(|b| b.is_ascii_digit())
}

/// The spellings of `path`: as given, canonical, and on Windows with either separator.
fn variants(path: &Path) -> Vec<String> {
    let mut v = vec![path.to_string_lossy().into_owned()];
    if let Ok(c) = std::fs::canonicalize(path) {
        let c = c.to_string_lossy().into_owned();
        // Windows' verbatim prefix is not how programs spell paths.
        v.push(c.strip_prefix(r"\\?\").map(str::to_owned).unwrap_or(c));
    }
    let mut more = Vec::new();
    for s in &v {
        if s.contains('\\') {
            more.push(s.replace('\\', "/"));
        }
    }
    v.extend(more);
    v.retain(|s| s.len() > 1);
    v.sort_by_key(|s| std::cmp::Reverse(s.len()));
    v.dedup();
    v
}

impl Scrubber {
    /// Scrub `roots` ((placeholder, path)); the longer paths are tried first.
    pub fn new(roots: &[(String, PathBuf)]) -> Self {
        let mut s = Self::default();
        for (placeholder, path) in roots {
            s.roots.push(Root {
                placeholder: placeholder.clone(),
                variants: variants(path),
            });
        }
        s
    }

    /// Also replace process id `pid`.
    pub fn add_pid(&mut self, pid: i64) {
        if pid > 0 && !self.pids.contains(&pid) {
            self.pids.push(pid);
        }
    }

    pub fn pids(&self) -> &[i64] {
        &self.pids
    }

    /// Also replace `text` by `placeholder` (a token of digits as a number value, any other wherever it stands
    /// alone in text).
    pub fn add_token(&mut self, placeholder: &str, text: &str) {
        let t = (placeholder.to_owned(), text.to_owned());
        if !text.is_empty() && !self.tokens.contains(&t) {
            self.tokens.push(t);
            self.tokens
                .sort_by_key(|(_, text)| std::cmp::Reverse(text.len()));
        }
    }

    /// Scrub every string and number of `v` in place.
    pub fn scrub(&self, v: &mut Value) {
        match v {
            Value::String(s) => {
                if let Some(t) = self.scrub_text(s) {
                    *s = t;
                }
            }
            Value::Number(n) => {
                if n.as_i64().is_some_and(|n| self.pids.contains(&n)) {
                    *v = Value::String("${PID}".into());
                } else if let Some((placeholder, _)) = self
                    .tokens
                    .iter()
                    .find(|(_, t)| numeric(t) && *t == n.to_string())
                {
                    *v = Value::String(placeholder.clone());
                }
            }
            Value::Array(a) => a.iter_mut().for_each(|x| self.scrub(x)),
            Value::Object(o) => o.values_mut().for_each(|x| self.scrub(x)),
            _ => {}
        }
    }

    /// `s` scrubbed, or `None` when nothing changed.
    pub fn scrub_text(&self, s: &str) -> Option<String> {
        let mut out = s.to_owned();
        let mut changed = false;
        let all: Vec<(&str, &str)> = {
            let mut all: Vec<(&str, &str)> = self
                .roots
                .iter()
                .flat_map(|r| {
                    r.variants
                        .iter()
                        .map(move |v| (v.as_str(), r.placeholder.as_str()))
                })
                .collect();
            all.sort_by_key(|(v, _)| std::cmp::Reverse(v.len()));
            all
        };
        for (path, placeholder) in all {
            if let Some(t) = replace_path(&out, path, placeholder) {
                out = t;
                changed = true;
            }
        }
        for pid in &self.pids {
            if *pid >= MIN_TEXT_PID
                && let Some(t) = replace_word(&out, &pid.to_string(), "${PID}")
            {
                out = t;
                changed = true;
            }
        }
        for (placeholder, text) in &self.tokens {
            if !numeric(text)
                && let Some(t) = replace_word(&out, text, placeholder)
            {
                out = t;
                changed = true;
            }
        }
        if let Some(t) = scrub_timestamps(&out) {
            out = t;
            changed = true;
        }
        changed.then_some(out)
    }
}

fn path_case_eq(a: &str, b: &str) -> bool {
    if cfg!(windows) {
        a.eq_ignore_ascii_case(b)
    } else {
        a == b
    }
}

/// Replace each occurrence of `path` in `s` that ends at a path boundary (end, a separator, or a character no path
/// name continues with) by `placeholder`, the rest of that path with `/` separators.
fn replace_path(s: &str, path: &str, placeholder: &str) -> Option<String> {
    if path.is_empty() || s.len() < path.len() {
        return None;
    }
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    let mut changed = false;
    let bytes = s.as_bytes();
    while i < s.len() {
        let rest = &s[i..];
        if rest.len() >= path.len()
            && rest.is_char_boundary(path.len())
            && path_case_eq(&rest[..path.len()], path)
        {
            let after = rest[path.len()..].chars().next();
            let boundary = match after {
                None => true,
                Some(c) => !(c.is_alphanumeric() || c == '_' || c == '-' || c == '.'),
            };
            let before_ok = i == 0 || !(bytes[i - 1].is_ascii_alphanumeric());
            if boundary && before_ok {
                out.push_str(placeholder);
                i += path.len();
                // The rest of this path, with `/` separators.
                while i < s.len() {
                    let c = s[i..].chars().next().expect("in bounds");
                    if c.is_whitespace() || c == '"' || c == '\'' || c == ',' || c == ';' {
                        break;
                    }
                    out.push(if c == '\\' { '/' } else { c });
                    i += c.len_utf8();
                }
                changed = true;
                continue;
            }
        }
        let c = rest.chars().next().expect("in bounds");
        out.push(c);
        i += c.len_utf8();
    }
    changed.then_some(out)
}

/// Replace `word` where no digit or letter touches it.
fn replace_word(s: &str, word: &str, with: &str) -> Option<String> {
    let mut out = String::with_capacity(s.len());
    let mut changed = false;
    let mut last = 0;
    let bytes = s.as_bytes();
    for (i, _) in s.match_indices(word) {
        if i < last {
            continue;
        }
        let before = i.checked_sub(1).map(|b| bytes[b]);
        let after = bytes.get(i + word.len()).copied();
        let touches = |b: Option<u8>| b.is_some_and(|b| b.is_ascii_alphanumeric());
        if touches(before) || touches(after) {
            continue;
        }
        out.push_str(&s[last..i]);
        out.push_str(with);
        last = i + word.len();
        changed = true;
    }
    if !changed {
        return None;
    }
    out.push_str(&s[last..]);
    Some(out)
}

/// Replace ISO 8601 timestamps (`YYYY-MM-DD[T ]HH:MM:SS[.fff][Z|±HH:MM]`) by `${TIME}`.
fn scrub_timestamps(s: &str) -> Option<String> {
    let b = s.as_bytes();
    if b.len() < 19 {
        return None;
    }
    let digits = |from: usize, n: usize| -> bool {
        b.get(from..from + n)
            .is_some_and(|d| d.iter().all(u8::is_ascii_digit))
    };
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    let mut last = 0;
    let mut changed = false;
    while i + 19 <= b.len() {
        let at = |k: usize, c: u8| b.get(i + k) == Some(&c);
        let ok = (i == 0 || !b[i - 1].is_ascii_digit())
            && digits(i, 4)
            && at(4, b'-')
            && digits(i + 5, 2)
            && at(7, b'-')
            && digits(i + 8, 2)
            && (at(10, b'T') || at(10, b' '))
            && digits(i + 11, 2)
            && at(13, b':')
            && digits(i + 14, 2)
            && at(16, b':')
            && digits(i + 17, 2);
        if !ok {
            i += 1;
            continue;
        }
        let mut end = i + 19;
        if b.get(end) == Some(&b'.') {
            end += 1;
            while b.get(end).is_some_and(u8::is_ascii_digit) {
                end += 1;
            }
        }
        if b.get(end) == Some(&b'Z') {
            end += 1;
        } else if matches!(b.get(end), Some(b'+' | b'-'))
            && digits(end + 1, 2)
            && b.get(end + 3) == Some(&b':')
            && digits(end + 4, 2)
        {
            end += 6;
        }
        out.push_str(&s[last..i]);
        out.push_str("${TIME}");
        last = end;
        i = end;
        changed = true;
    }
    if !changed {
        return None;
    }
    out.push_str(&s[last..]);
    Some(out)
}

/// Puts this run's values back where a recording has placeholders (the replayer, on the adapter's messages).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Substitution {
    /// (placeholder, this run's path).
    pub roots: Vec<(String, PathBuf)>,
    /// What `${PID}` becomes.
    pub pid: i64,
    /// (placeholder, this run's text) for the tokens the recording has.
    pub tokens: Vec<(String, String)>,
}

impl Substitution {
    pub fn apply(&self, v: &mut Value) {
        match v {
            Value::String(s) if s == "${PID}" => *v = json!(self.pid),
            Value::String(s) if self.number_of(s).is_some() => {
                *v = json!(self.number_of(s).unwrap_or_default());
            }
            Value::String(s) => {
                if let Some(t) = self.apply_text(s) {
                    *s = t;
                }
            }
            Value::Array(a) => a.iter_mut().for_each(|x| self.apply(x)),
            Value::Object(o) => o.values_mut().for_each(|x| self.apply(x)),
            _ => {}
        }
    }

    /// The number a numeric token's placeholder stands for.
    fn number_of(&self, s: &str) -> Option<i64> {
        self.tokens
            .iter()
            .find(|(p, t)| p == s && numeric(t))
            .and_then(|(_, t)| t.parse().ok())
    }

    /// `s` with the placeholders replaced (the path after one with this platform's separator), or `None`.
    pub fn apply_text(&self, s: &str) -> Option<String> {
        if !s.contains("${") {
            return None;
        }
        let mut out = s.to_owned();
        for (placeholder, path) in &self.roots {
            if !out.contains(placeholder.as_str()) {
                continue;
            }
            let value = path.to_string_lossy();
            let mut next = String::with_capacity(out.len());
            let mut rest = out.as_str();
            while let Some(at) = rest.find(placeholder.as_str()) {
                next.push_str(&rest[..at]);
                next.push_str(&value);
                rest = &rest[at + placeholder.len()..];
                let end = rest
                    .find(|c: char| {
                        c.is_whitespace() || c == '"' || c == '\'' || c == ',' || c == ';'
                    })
                    .unwrap_or(rest.len());
                let tail = &rest[..end];
                if std::path::MAIN_SEPARATOR == '/' {
                    next.push_str(tail);
                } else {
                    next.push_str(&tail.replace('/', std::path::MAIN_SEPARATOR_STR));
                }
                rest = &rest[end..];
            }
            next.push_str(rest);
            out = next;
        }
        if out.contains("${PID}") {
            out = out.replace("${PID}", &self.pid.to_string());
        }
        for (placeholder, text) in &self.tokens {
            if out.contains(placeholder.as_str()) {
                out = out.replace(placeholder.as_str(), text);
            }
        }
        (out != s).then_some(out)
    }
}

/// What to record and where.
#[derive(Debug, Clone)]
pub struct RecordOptions {
    /// The `.dap.json` file to write.
    pub path: PathBuf,
    pub adapter: String,
    pub version: String,
    /// (placeholder, path): `${ROOT}` the repository, `${TMP}` the scenario's temporary directory, others as needed.
    pub roots: Vec<(String, PathBuf)>,
}

#[derive(Default)]
struct Log {
    messages: Vec<(u64, Dir, Value, Option<u64>)>,
    ended: Option<Ended>,
    pids: Vec<i64>,
    tokens: Vec<(String, String)>,
}

impl Log {
    fn scrubber(&self, roots: &[(String, PathBuf)]) -> Scrubber {
        let mut s = Scrubber::new(roots);
        for p in &self.pids {
            s.add_pid(*p);
        }
        for (placeholder, text) in &self.tokens {
            s.add_token(placeholder, text);
        }
        s
    }
}

struct Shared {
    options: RecordOptions,
    description: String,
    start: Instant,
    log: Mutex<Log>,
    /// The counter shared by the connections of one server.
    order: Option<Arc<AtomicU64>>,
}

impl Shared {
    fn next_order(&self) -> Option<u64> {
        self.order
            .as_ref()
            .map(|o| o.fetch_add(1, Ordering::SeqCst))
    }
}

impl Shared {
    fn push(&self, dir: Dir, body: &[u8]) {
        let t = self.start.elapsed().as_millis() as u64;
        let Ok(message) = serde_json::from_slice::<Value>(body) else {
            return;
        };
        let mut log = self.log.lock().unwrap_or_else(|e| e.into_inner());
        // The process ids this session names.
        let pid = match (dir, message["type"].as_str()) {
            (Dir::Adapter, Some("event")) if message["event"] == "process" => {
                message["body"]["systemProcessId"].as_i64()
            }
            (Dir::Client, Some("request")) if message["command"] == "attach" => {
                message["arguments"]["processId"]
                    .as_i64()
                    .or_else(|| message["arguments"]["pid"].as_i64())
            }
            _ => None,
        };
        if let Some(p) = pid
            && !log.pids.contains(&p)
        {
            log.pids.push(p);
        }
        let order = self.next_order();
        log.messages.push((t, dir, message, order));
    }

    /// One side ended the session: write the file now (it is written again when both halves are gone).
    fn end(&self, by: Ended) {
        {
            let mut log = self.log.lock().unwrap_or_else(|e| e.into_inner());
            if log.ended.is_some() {
                return;
            }
            log.ended = Some(by);
        }
        if let Err(e) = self.write() {
            eprintln!(
                "eludite-dap: cannot write the recording {}: {e}",
                self.options.path.display()
            );
        }
    }

    fn recording(&self) -> Recording {
        let log = self.log.lock().unwrap_or_else(|e| e.into_inner());
        let scrubber = log.scrubber(&self.options.roots);
        let messages = log
            .messages
            .iter()
            .map(|(t, dir, m, order)| {
                let mut message = m.clone();
                truncate_variables(&mut message);
                scrubber.scrub(&mut message);
                RecordedMessage {
                    t_ms: *t,
                    dir: *dir,
                    message,
                    order: *order,
                }
            })
            .collect();
        let description = scrubber
            .scrub_text(&self.description)
            .unwrap_or_else(|| self.description.clone());
        Recording {
            adapter: self.options.adapter.clone(),
            version: self.options.version.clone(),
            recorded_at: utc_now(),
            platform: platform(),
            description,
            ended: log.ended,
            messages,
        }
    }

    fn write(&self) -> io::Result<Recording> {
        let r = self.recording();
        r.write(&self.options.path)?;
        Ok(r)
    }
}

impl Drop for Shared {
    fn drop(&mut self) {
        if let Err(e) = self.write() {
            eprintln!(
                "eludite-dap: cannot write the recording {}: {e}",
                self.options.path.display()
            );
        }
    }
}

/// Keep the first [`MAX_RECORDED_VARIABLES`] rows of a `variables` answer.
fn truncate_variables(message: &mut Value) {
    if message["type"] != "response" || message["command"] != "variables" {
        return;
    }
    let Some(rows) = message
        .get_mut("body")
        .and_then(|b| b.get_mut("variables"))
        .and_then(Value::as_array_mut)
    else {
        return;
    };
    if rows.len() > MAX_RECORDED_VARIABLES {
        rows.truncate(MAX_RECORDED_VARIABLES);
        message["truncated_by_recorder"] = json!(true);
    }
}

/// The recording of one connection: write it now (it is also written when the connection's both halves are gone).
#[derive(Clone)]
pub struct RecordHandle {
    shared: Arc<Shared>,
}

impl RecordHandle {
    /// Scrub `pid` too (a process the test started itself and attaches to).
    pub fn add_pid(&self, pid: i64) {
        let mut log = self.shared.log.lock().unwrap_or_else(|e| e.into_inner());
        if !log.pids.contains(&pid) {
            log.pids.push(pid);
        }
    }

    /// Scrub `text` as `placeholder` too (the run's own value: a page's origin, a DevTools port, a target id).
    pub fn add_token(&self, placeholder: &str, text: &str) {
        let mut log = self.shared.log.lock().unwrap_or_else(|e| e.into_inner());
        let t = (placeholder.to_owned(), text.to_owned());
        if !log.tokens.contains(&t) {
            log.tokens.push(t);
        }
    }

    /// Record the test's action `label` outside DAP (a click in the page) here, between the messages before and
    /// after it: the replayer holds the adapter's later messages until the test passes the same mark.
    pub fn mark(&self, label: &str) {
        let t = self.shared.start.elapsed().as_millis() as u64;
        let mut log = self.shared.log.lock().unwrap_or_else(|e| e.into_inner());
        let order = self.shared.next_order();
        log.messages
            .push((t, Dir::Client, json!({"type": MARK, "label": label}), order));
    }

    /// The scrubber as it stands (the roots, the process ids seen so far and the tokens), for scrubbing what else
    /// the test records (its goldens) by the same rules.
    pub fn scrubber(&self) -> Scrubber {
        let log = self.shared.log.lock().unwrap_or_else(|e| e.into_inner());
        log.scrubber(&self.shared.options.roots)
    }

    /// Write the file with everything so far and return what was written.
    pub fn write(&self) -> io::Result<Recording> {
        self.shared.write()
    }

    pub fn path(&self) -> &Path {
        &self.shared.options.path
    }

    /// Messages logged so far.
    pub fn len(&self) -> usize {
        self.shared
            .log
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .messages
            .len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Whether either side has ended the session.
    pub fn ended(&self) -> Option<Ended> {
        self.shared
            .log
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .ended
    }
}

/// Splits a byte stream into `Content-Length` framed bodies.
#[derive(Default)]
struct Frames {
    buf: Vec<u8>,
}

impl Frames {
    fn push(&mut self, bytes: &[u8], mut each: impl FnMut(&[u8])) {
        self.buf.extend_from_slice(bytes);
        loop {
            let Some(head_end) = self.buf.windows(4).position(|w| w == b"\r\n\r\n") else {
                return;
            };
            let head = String::from_utf8_lossy(&self.buf[..head_end]).into_owned();
            let len = head.lines().find_map(|l| {
                let (k, v) = l.split_once(':')?;
                k.trim()
                    .eq_ignore_ascii_case("content-length")
                    .then(|| v.trim().parse::<usize>().ok())
                    .flatten()
            });
            let Some(len) = len else {
                // Not DAP framing: drop the header and go on.
                self.buf.drain(..head_end + 4);
                continue;
            };
            let start = head_end + 4;
            if self.buf.len() < start + len {
                return;
            }
            each(&self.buf[start..start + len]);
            self.buf.drain(..start + len);
        }
    }
}

struct RecordingReader {
    inner: Box<dyn Read + Send>,
    frames: Frames,
    shared: Arc<Shared>,
}

impl Read for RecordingReader {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        let n = self.inner.read(out)?;
        if n == 0 {
            self.shared.end(Ended::Adapter);
        } else {
            let shared = &self.shared;
            self.frames
                .push(&out[..n], |body| shared.push(Dir::Adapter, body));
        }
        Ok(n)
    }
}

struct RecordingWriter {
    inner: Box<dyn Write + Send>,
    frames: Frames,
    shared: Arc<Shared>,
}

impl Write for RecordingWriter {
    /// Log first, then send all of it: the adapter's answer can never be logged before the request it answers.
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let shared = &self.shared;
        self.frames
            .push(bytes, |body| shared.push(Dir::Client, body));
        self.inner.write_all(bytes)?;
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

impl Drop for RecordingWriter {
    fn drop(&mut self) {
        self.shared.end(Ended::Client);
    }
}

/// Wrap `connection` so every message in both directions is recorded to `options.path`. The file is written when
/// either side ends the session (the adapter closes its output, the client hangs up), again when both halves of the
/// connection are dropped (the client's threads end), and whenever [`RecordHandle::write`] is called. The adapter's stderr, child process and shutdown pass through untouched.
pub fn record(connection: Connection, options: RecordOptions) -> (Connection, RecordHandle) {
    record_with(connection, options, None)
}

/// [`record`], numbering the messages with `order`, the counter every connection of one adapter server shares (a
/// vscode-js-debug server's sessions, brief 0038): the replay keeps their order across connections.
pub fn record_ordered(
    connection: Connection,
    options: RecordOptions,
    order: Arc<AtomicU64>,
) -> (Connection, RecordHandle) {
    record_with(connection, options, Some(order))
}

fn record_with(
    connection: Connection,
    options: RecordOptions,
    order: Option<Arc<AtomicU64>>,
) -> (Connection, RecordHandle) {
    let Connection {
        reader,
        writer,
        child,
        stderr,
        shutdown,
        description,
    } = connection;
    let shared = Arc::new(Shared {
        options,
        description: description.clone(),
        start: Instant::now(),
        log: Mutex::default(),
        order,
    });
    if let Some(c) = &child {
        // The adapter's own process id, should it appear.
        shared
            .log
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .pids
            .push(i64::from(c.id()));
    }
    let conn = Connection {
        reader: Box::new(RecordingReader {
            inner: reader,
            frames: Frames::default(),
            shared: shared.clone(),
        }),
        writer: Box::new(RecordingWriter {
            inner: writer,
            frames: Frames::default(),
            shared: shared.clone(),
        }),
        child,
        stderr,
        shutdown,
        description,
    };
    (conn, RecordHandle { shared })
}

/// A message as the re-record check compares it: without `seq` and `request_seq` (they follow the interleaving),
/// lldb-dap's `statistics` (its own memory figures), and the code and memory addresses anywhere in it
/// (`instructionReference`, `instructionPointerReference`, `memoryReference`: the debuggee's layout, which the
/// machine that built it decides). In a `stackTrace` answer, the frames of the C runtime (no source under a
/// recorded root or the toolchain's `/rustc/`) are one `runtime` placeholder per run of them, and `totalFrames`
/// goes: whether glibc's start-up frames have lines and columns, or how many there are, is the machine's debug
/// information. A `scopes` answer loses its variable counts for the same reason (lldb's Globals).
fn comparable(m: &Value) -> Value {
    fn is_program_frame(f: &Value) -> bool {
        f["source"]["path"]
            .as_str()
            .is_some_and(|p| p.contains("${") || p.starts_with("/rustc/"))
    }
    fn collapse_runtime_frames(body: &mut Value) {
        let Some(frames) = body["stackFrames"].as_array() else {
            return;
        };
        let mut kept = Vec::with_capacity(frames.len());
        for f in frames {
            if is_program_frame(f) {
                kept.push(f.clone());
            } else if kept.last().is_none_or(|k: &Value| k["runtime"] != true) {
                kept.push(json!({"runtime": true}));
            }
        }
        body["stackFrames"] = Value::Array(kept);
        if let Some(o) = body.as_object_mut() {
            o.remove("totalFrames");
        }
    }
    fn strip(v: &mut Value) {
        match v {
            Value::Object(o) => {
                o.remove("instructionReference");
                o.remove("instructionPointerReference");
                o.remove("memoryReference");
                o.values_mut().for_each(strip);
            }
            Value::Array(a) => a.iter_mut().for_each(strip),
            _ => {}
        }
    }
    let mut m = m.clone();
    if let Some(o) = m.as_object_mut() {
        o.remove("seq");
        o.remove("request_seq");
        o.remove("statistics");
    }
    if m["type"] == "response" && m["command"] == "stackTrace" {
        collapse_runtime_frames(&mut m["body"]);
    }
    if m["type"] == "response"
        && m["command"] == "scopes"
        && let Some(scopes) = m["body"]["scopes"].as_array_mut()
    {
        // How many globals lldb sees depends on the machine's debug information (glibc's, or none).
        for scope in scopes.iter_mut().filter_map(Value::as_object_mut) {
            scope.remove("namedVariables");
            scope.remove("indexedVariables");
        }
    }
    strip(&mut m);
    m
}

/// The messages the re-record check compares, in groups that each keep their order: the client's requests; the
/// adapter's responses; its events other than `output` and `continued`; and the text of its `output` events, per
/// category, joined (how the debuggee's writes are cut into events is timing). `continued` events are left out
/// (whether lldb-dap reports one depends on how fast the debuggee stops again).
///
/// The end of the session (from the client's first `disconnect` or `terminate`, or the adapter's `terminated` event,
/// whichever comes first) is its own group, `adapter at the end`, compared as a set: lldb-dap 18 sends `exited` and
/// `terminated` and answers `disconnect` in either order when the client ends the session. Its `output` there is left
/// out: lldb-dap 18 aborts as it exits and prints a crash report with this run's addresses, in pieces.
fn groups(r: &Recording) -> BTreeMap<String, Vec<Value>> {
    let mut out: BTreeMap<String, Vec<Value>> = BTreeMap::new();
    let mut text: BTreeMap<String, String> = BTreeMap::new();
    let mut ending = false;
    for m in &r.messages {
        let msg = &m.message;
        let is_event = msg["type"] == "event";
        if (m.dir == Dir::Client
            && matches!(msg["command"].as_str(), Some("disconnect" | "terminate")))
            || (m.dir == Dir::Adapter && is_event && msg["event"] == "terminated")
        {
            ending = true;
        }
        let group = match m.dir {
            Dir::Client => "client",
            // A script's load is reported as it is parsed, in no fixed order with the stop (vscode-js-debug's
            // `loadedSource`; the shell does not use it): timing.
            Dir::Adapter
                if is_event
                    && matches!(msg["event"].as_str(), Some("continued" | "loadedSource")) =>
            {
                continue;
            }
            Dir::Adapter if ending && is_event && msg["event"] == "output" => continue,
            Dir::Adapter if ending => "adapter at the end",
            Dir::Adapter if is_event && msg["event"] == "output" => {
                let category = msg["body"]["category"].as_str().unwrap_or("console");
                text.entry(format!("adapter output ({category})"))
                    .or_default()
                    .push_str(msg["body"]["output"].as_str().unwrap_or_default());
                continue;
            }
            Dir::Adapter if is_event => "adapter events",
            Dir::Adapter => "adapter responses",
        };
        out.entry(group.to_owned())
            .or_default()
            .push(comparable(msg));
    }
    if let Some(end) = out.get_mut("adapter at the end") {
        end.sort_by_cached_key(Value::to_string);
    }
    for (k, t) in text {
        out.insert(
            k,
            vec![json!({"type": "event", "event": "output", "text": t})],
        );
    }
    out
}

/// The first difference between two JSON values, as `path: expected X, got Y`.
pub fn first_difference(expected: &Value, actual: &Value) -> Option<String> {
    differences(expected, actual, 1).into_iter().next()
}

/// Up to `max` differences between two JSON values, each as `path: expected X, got Y`.
pub fn differences(expected: &Value, actual: &Value, max: usize) -> Vec<String> {
    fn short(v: &Value) -> String {
        let s = v.to_string();
        if s.chars().count() > 160 {
            format!("{}…", s.chars().take(160).collect::<String>())
        } else {
            s
        }
    }
    fn walk(e: &Value, a: &Value, path: &str, out: &mut Vec<String>, max: usize) {
        if out.len() >= max || e == a {
            return;
        }
        match (e, a) {
            (Value::Object(eo), Value::Object(ao)) => {
                let keys: std::collections::BTreeSet<&String> =
                    eo.keys().chain(ao.keys()).collect();
                for k in keys {
                    let p = format!("{path}.{k}");
                    match (eo.get(k), ao.get(k)) {
                        (Some(x), Some(y)) => walk(x, y, &p, out, max),
                        (Some(x), None) => out.push(format!("{p}: expected {}, missing", short(x))),
                        (None, Some(y)) => out.push(format!("{p}: unexpected {}", short(y))),
                        (None, None) => {}
                    }
                    if out.len() >= max {
                        return;
                    }
                }
            }
            (Value::Array(ea), Value::Array(aa)) => {
                for i in 0..ea.len().max(aa.len()) {
                    let p = format!("{path}[{i}]");
                    match (ea.get(i), aa.get(i)) {
                        (Some(x), Some(y)) => walk(x, y, &p, out, max),
                        (Some(x), None) => out.push(format!("{p}: expected {}, missing", short(x))),
                        (None, Some(y)) => out.push(format!("{p}: unexpected {}", short(y))),
                        (None, None) => {}
                    }
                    if out.len() >= max {
                        return;
                    }
                }
            }
            _ => out.push(format!(
                "{}: expected {}, got {}",
                if path.is_empty() { "$" } else { path },
                short(e),
                short(a)
            )),
        }
    }
    let mut out = Vec::new();
    walk(expected, actual, "", &mut out, max);
    out
}

/// The re-record check: `rerecorded` reproduces `checked_in` when each group of messages (see `groups`: the
/// client's requests, the adapter's responses, its events, its output text by category, the session's end) agrees,
/// ignoring `t_ms`, `recorded_at`, `seq`, `request_seq`, `order`, lldb-dap's `statistics`, the addresses in
/// `instructionReference`, `instructionPointerReference` and `memoryReference` (the debuggee's layout differs
/// between the machines that build it), the C runtime's stack frames (see `comparable`), `continued` events and
/// vscode-js-debug's `loadedSource` events: how the directions, the adapter's events and the debuggee's streams
/// interleave is timing. `Err` names the first difference of each group that differs.
pub fn compare(checked_in: &Recording, rerecorded: &Recording) -> Result<(), String> {
    let mut problems = Vec::new();
    if checked_in.adapter != rerecorded.adapter {
        problems.push(format!(
            "adapter: {} became {}",
            checked_in.adapter, rerecorded.adapter
        ));
    }
    if checked_in.version != rerecorded.version {
        problems.push(format!(
            "version: {} became {}",
            checked_in.version, rerecorded.version
        ));
    }
    let (a_groups, b_groups) = (groups(checked_in), groups(rerecorded));
    let names: std::collections::BTreeSet<&String> =
        a_groups.keys().chain(b_groups.keys()).collect();
    let empty = Vec::new();
    for name in names {
        let a = a_groups.get(name).unwrap_or(&empty);
        let b = b_groups.get(name).unwrap_or(&empty);
        let mut differed = false;
        for (i, (x, y)) in a.iter().zip(b).enumerate() {
            if x != y {
                problems.push(format!(
                    "{name} message {} ({}): {}",
                    i + 1,
                    label(x),
                    first_difference(x, y).unwrap_or_default()
                ));
                differed = true;
                break;
            }
        }
        if a.len() != b.len() && !differed {
            let n = a.len().min(b.len());
            problems.push(format!(
                "{name} messages: {} checked in, {} re-recorded; the first extra is {}",
                a.len(),
                b.len(),
                a.get(n).or_else(|| b.get(n)).map(label).unwrap_or_default()
            ));
        }
    }
    if problems.is_empty() {
        Ok(())
    } else {
        Err(problems.join("\n"))
    }
}

/// `request launch`, `response stackTrace`, `event stopped`.
pub fn label(m: &Value) -> String {
    let kind = m["type"].as_str().unwrap_or("?");
    let what = m["command"]
        .as_str()
        .or_else(|| m["event"].as_str())
        .unwrap_or("?");
    format!("{kind} {what}")
}

/// Count of each request command in `r`, for summaries.
pub fn request_counts(r: &Recording) -> BTreeMap<String, usize> {
    let mut m = BTreeMap::new();
    for (c, _) in r.requests() {
        *m.entry(c).or_default() += 1;
    }
    m
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_pids_and_times_are_scrubbed_and_substituted_back() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("repo");
        let scratch = tmp.path().join("scenario");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(&scratch).unwrap();
        let mut s = Scrubber::new(&[
            ("${ROOT}".into(), root.clone()),
            ("${TMP}".into(), scratch.clone()),
        ]);
        s.add_pid(43_210);
        let r = root.to_string_lossy();
        let t = scratch.to_string_lossy();
        let mut v = json!({
            "source": {"path": format!("{r}/src/Program.cs")},
            "cwd": t,
            "other": format!("{t}x/not-under"),
            "systemProcessId": 43_210,
            "line": 432,
            "output": format!("[43210] exited at 2026-10-03T12:34:56.789Z in {t}/bin 143210\n"),
        });
        s.scrub(&mut v);
        assert_eq!(v["source"]["path"], "${ROOT}/src/Program.cs");
        assert_eq!(v["cwd"], "${TMP}");
        assert_eq!(
            v["other"],
            format!("{t}x/not-under"),
            "a longer name is not under the root"
        );
        assert_eq!(v["systemProcessId"], "${PID}");
        assert_eq!(v["line"], 432);
        assert_eq!(
            v["output"],
            "[${PID}] exited at ${TIME} in ${TMP}/bin 143210\n"
        );
        let sub = Substitution {
            roots: vec![
                ("${ROOT}".into(), PathBuf::from("/elsewhere/repo")),
                ("${TMP}".into(), PathBuf::from("/t/s")),
            ],
            pid: 77,
            tokens: Vec::new(),
        };
        sub.apply(&mut v);
        let sep = std::path::MAIN_SEPARATOR;
        assert_eq!(
            v["source"]["path"],
            format!("/elsewhere/repo{sep}src{sep}Program.cs")
        );
        assert_eq!(v["systemProcessId"], 77);
        assert_eq!(
            v["output"],
            format!("[77] exited at ${{TIME}} in /t/s{sep}bin 143210\n")
        );
    }

    /// Brief 0038: a page's origin and target id are scrubbed in text, a DevTools port as a number only (never
    /// inside text), and substituted back with the replay's own values.
    #[test]
    fn tokens_are_scrubbed_and_substituted_back() {
        let mut s = Scrubber::default();
        s.add_token("${ORIGIN}", "127.0.0.1:43123");
        s.add_token("${TARGET}", "6A1F9E0C2B7D");
        s.add_token("${DEVTOOLS_PORT}", "40211");
        let mut v = json!({
            "url": "http://127.0.0.1:43123/app.js",
            "longer": "http://127.0.0.1:431234/",
            "port": 40211,
            "text": "port 40211 and line 40211",
            "targetId": "6A1F9E0C2B7D",
            "line": 25,
        });
        s.scrub(&mut v);
        assert_eq!(v["url"], "http://${ORIGIN}/app.js");
        assert_eq!(
            v["longer"], "http://127.0.0.1:431234/",
            "a longer port is another"
        );
        assert_eq!(v["port"], "${DEVTOOLS_PORT}");
        assert_eq!(
            v["text"], "port 40211 and line 40211",
            "a number token never in text"
        );
        assert_eq!(v["targetId"], "${TARGET}");
        assert_eq!(v["line"], 25);
        let sub = Substitution {
            tokens: vec![
                ("${ORIGIN}".into(), "127.0.0.1:5180".into()),
                ("${TARGET}".into(), "TARGET-P1".into()),
                ("${DEVTOOLS_PORT}".into(), "9".into()),
            ],
            ..Substitution::default()
        };
        sub.apply(&mut v);
        assert_eq!(v["url"], "http://127.0.0.1:5180/app.js");
        assert_eq!(v["port"], 9);
        assert_eq!(v["targetId"], "TARGET-P1");
    }

    #[test]
    fn a_large_variables_answer_keeps_fifty_rows_and_says_so() {
        let rows: Vec<Value> = (0..201)
            .map(|i| json!({"name": format!("l{i}"), "value": "0", "variablesReference": 0}))
            .collect();
        let mut m = json!({"type": "response", "command": "variables", "seq": 3, "request_seq": 2, "success": true, "body": {"variables": rows}});
        truncate_variables(&mut m);
        assert_eq!(m["body"]["variables"].as_array().unwrap().len(), 50);
        assert_eq!(m["truncated_by_recorder"], true);
        let mut small = json!({"type": "response", "command": "variables", "body": {"variables": [{"name": "a"}]}});
        truncate_variables(&mut small);
        assert!(small.get("truncated_by_recorder").is_none());
    }

    #[test]
    fn frames_split_across_writes_are_found_whole() {
        let mut f = Frames::default();
        let mut got = Vec::new();
        let a = b"{\"seq\":1}";
        let b = b"{\"seq\":2}";
        let mut bytes = format!("Content-Length: {}\r\n\r\n", a.len()).into_bytes();
        bytes.extend_from_slice(a);
        bytes.extend_from_slice(format!("Content-Length: {}\r\n\r\n", b.len()).as_bytes());
        bytes.extend_from_slice(b);
        for chunk in bytes.chunks(5) {
            f.push(chunk, |body| got.push(body.to_vec()));
        }
        assert_eq!(got, vec![a.to_vec(), b.to_vec()]);
    }

    #[test]
    fn the_text_has_one_message_per_line_and_reads_back() {
        let r = Recording {
            adapter: "fake".into(),
            version: "1".into(),
            recorded_at: utc_now(),
            platform: platform(),
            description: "fake adapter".into(),
            ended: Some(Ended::Adapter),
            messages: vec![
                RecordedMessage {
                    t_ms: 0,
                    dir: Dir::Client,
                    message: json!({"seq": 1, "type": "request", "command": "initialize"}),
                    order: None,
                },
                RecordedMessage {
                    t_ms: 2,
                    dir: Dir::Adapter,
                    message: json!({"seq": 1, "type": "response", "request_seq": 1, "command": "initialize", "success": true}),
                    order: None,
                },
            ],
        };
        let text = r.to_text();
        assert_eq!(text.lines().count(), 12, "{text}");
        let back: Recording = serde_json::from_str(&text).unwrap();
        assert_eq!(back, r);
        assert!(
            r.recorded_at.ends_with('Z') && r.recorded_at.len() == 20,
            "{}",
            r.recorded_at
        );
        let mut other = r.clone();
        other.messages[1].t_ms = 900;
        other.messages[1].message["seq"] = json!(7);
        assert_eq!(compare(&r, &other), Ok(()), "timing and seq are not drift");
        let (mut here, mut there) = (r.clone(), r.clone());
        here.messages[1].message["body"] =
            json!({"breakpoints": [{"verified": true, "instructionReference": "0x55555556C94C"}]});
        there.messages[1].message["body"] =
            json!({"breakpoints": [{"verified": true, "instructionReference": "0x55555556C24C"}]});
        assert_eq!(
            compare(&here, &there),
            Ok(()),
            "addresses are the building machine's"
        );
        let stack = |libc: Value| {
            let mut r = r.clone();
            r.messages[1].message["command"] = json!("stackTrace");
            r.messages[1].message["body"] = json!({"totalFrames": 4, "stackFrames": [
                {"id": 1, "name": "app::main", "line": 3, "column": 5, "source": {"path": "${TMP}/src/main.rs"}},
                {"id": 2, "name": "std::rt::lang_start", "line": 9, "column": 1, "source": {"path": "/rustc/abc/library/std/src/rt.rs"}},
                {"id": 3, "name": "main", "line": 0, "column": 0},
                libc,
            ]});
            r
        };
        let with_debug_info = stack(
            json!({"id": 4, "name": "__libc_start_call_main", "line": 58, "column": 16, "source": {"path": "sysdeps/nptl/libc_start_call_main.h"}}),
        );
        let without =
            stack(json!({"id": 4, "name": "__libc_start_call_main", "line": 0, "column": 0}));
        assert_eq!(
            compare(&with_debug_info, &without),
            Ok(()),
            "the C runtime's frames are the machine's"
        );
        let mut moved = without.clone();
        moved.messages[1].message["body"]["stackFrames"][0]["line"] = json!(4);
        assert!(compare(&with_debug_info, &moved).is_err());
        let scopes = |globals: u32| {
            let mut r = r.clone();
            r.messages[1].message["command"] = json!("scopes");
            r.messages[1].message["body"] = json!({"scopes": [
                {"name": "Locals", "namedVariables": 1, "variablesReference": 1},
                {"name": "Globals", "namedVariables": globals, "variablesReference": 2},
            ]});
            r
        };
        assert_eq!(
            compare(&scopes(0), &scopes(2)),
            Ok(()),
            "the globals lldb sees are the machine's"
        );
        other.messages[1].message["success"] = json!(false);
        let e = compare(&r, &other).unwrap_err();
        assert!(
            e.contains(
                "adapter responses message 1 (response initialize): .success: expected true, got false"
            ),
            "{e}"
        );
    }

    #[test]
    fn the_re_record_check_ignores_timing_and_catches_drift() {
        let m = |t: u64, dir: Dir, message: Value| RecordedMessage {
            t_ms: t,
            dir,
            message,
            order: None,
        };
        let out = |text: &str| json!({"type": "event", "event": "output", "body": {"category": "stdout", "output": text}});
        let base = |outputs: Vec<Value>, end: Vec<Value>| {
            let mut messages = vec![
                m(
                    0,
                    Dir::Client,
                    json!({"seq": 1, "type": "request", "command": "continue"}),
                ),
                m(
                    1,
                    Dir::Adapter,
                    json!({"seq": 1, "type": "response", "request_seq": 1, "command": "continue", "success": true}),
                ),
            ];
            messages.extend(outputs.into_iter().map(|o| m(2, Dir::Adapter, o)));
            messages.push(m(
                3,
                Dir::Client,
                json!({"seq": 2, "type": "request", "command": "disconnect"}),
            ));
            messages.extend(end.into_iter().map(|e| m(4, Dir::Adapter, e)));
            Recording {
                adapter: "lldb".into(),
                version: "18".into(),
                recorded_at: utc_now(),
                platform: platform(),
                description: String::new(),
                ended: Some(Ended::Adapter),
                messages,
            }
        };
        let exited = json!({"type": "event", "event": "exited", "body": {"exitCode": 9}});
        let terminated = json!({"type": "event", "event": "terminated"});
        let answered = json!({"type": "response", "command": "disconnect", "success": true});
        let a = base(
            vec![out("hello "), out("world\n")],
            vec![terminated.clone(), answered.clone(), exited.clone()],
        );
        let b = base(
            vec![
                out("hello world\n"),
                json!({"type": "event", "event": "continued"}),
                json!({"type": "event", "event": "loadedSource", "body": {"reason": "new", "source": {"name": "app.js"}}}),
            ],
            vec![
                exited.clone(),
                terminated.clone(),
                answered.clone(),
                out("free(): invalid pointer at 0x7f00\n"),
            ],
        );
        assert_eq!(
            compare(&a, &b),
            Ok(()),
            "how output is cut, the end's order, a script's load and the crash are timing"
        );
        let c = base(
            vec![out("hello there\n")],
            vec![terminated.clone(), answered.clone(), exited.clone()],
        );
        let e = compare(&a, &c).unwrap_err();
        assert!(e.contains("adapter output (stdout)"), "{e}");
        let d = base(vec![out("hello world\n")], vec![terminated, answered]);
        let e = compare(&a, &d).unwrap_err();
        assert!(e.contains("adapter at the end"), "{e}");
    }
}
