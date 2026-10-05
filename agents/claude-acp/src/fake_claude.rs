//! A fake `claude` for tests: it replays a recorded, redacted stream-json
//! session (see `tools/record.py` and `tools/redact.py`), built as the
//! `eludite-fake-claude` binary. Not used by the adapter itself.
//!
//! Fixture lines are `{"t_ms", "dir": "in"|"out"|"exit", "m"|"code"}`. The
//! fake writes `out` messages in order. At each `in` it reads one line from
//! stdin and checks it against the recording: same `type`; for a
//! `control_request` the same `subtype` (its `request_id` is remembered, and
//! the recorded reply is sent under the caller's id); for a `control_response`
//! to a `can_use_tool` request the same `request_id` and the same `behavior`.
//! A mismatch is logged and the fake exits with status 3, unless
//! `FAKE_CLAUDE_LENIENT=1`. `{{SESSION_ID}}` and `{{CWD}}` are replaced by the
//! `--session-id` (or `--resume`) argument and the working directory.
//!
//! Brief 0060: a fixture of several `claude` processes has a
//! `{"dir": "start", "m": {"flag", "label"}}` record before each one's; the
//! fake replays the first for `--session-id` and, for `--resume`, the one
//! labelled `$FAKE_CLAUDE_RESUME` (default `resume`; the recording's refusal is
//! `refused`). `{"dir": "err", "m": {"line"}}` records are written to stderr.
//!
//! Environment:
//! - `FAKE_CLAUDE_FIXTURE`: the fixture (required unless the scenario is `stream`);
//! - `FAKE_CLAUDE_LOG`: append a JSON line per event (argv, environment
//!   checks, MCP config, each input line, mismatches);
//! - `FAKE_CLAUDE_VERSION`: what `--version` prints (default `2.1.287 (Claude Code)`);
//! - `FAKE_CLAUDE_SCENARIO=stream` with `FAKE_CLAUDE_CHUNKS` and
//!   `FAKE_CLAUDE_RATE`: answer `initialize`, then stream that many text
//!   deltas per user message at that rate (the hot-path benchmark).

use std::collections::HashMap;
use std::io::{self, BufRead, Write};
use std::path::Path;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

/// Environment variables the fake reads.
pub const FIXTURE_ENV: &str = "FAKE_CLAUDE_FIXTURE";
pub const LOG_ENV: &str = "FAKE_CLAUDE_LOG";
pub const VERSION_ENV: &str = "FAKE_CLAUDE_VERSION";
pub const LENIENT_ENV: &str = "FAKE_CLAUDE_LENIENT";
pub const SCENARIO_ENV: &str = "FAKE_CLAUDE_SCENARIO";
/// Which recorded process answers a `--resume` (brief 0060): its start record's label.
pub const RESUME_ENV: &str = "FAKE_CLAUDE_RESUME";

struct Log(Option<std::fs::File>);

impl Log {
    fn open() -> Self {
        Log(std::env::var_os(LOG_ENV).and_then(|p| {
            std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(p)
                .ok()
        }))
    }
    fn write(&mut self, v: Value) {
        if let Some(f) = &mut self.0 {
            // One write per line, so a reader never sees half a record.
            let _ = f.write_all(format!("{v}\n").as_bytes());
        }
    }
}

fn arg_after(args: &[String], flag: &str) -> Option<String> {
    args.iter()
        .position(|a| a == flag)
        .and_then(|i| args.get(i + 1))
        .cloned()
}

fn substitute(v: &Value, sid: &str, cwd: &str) -> Value {
    match v {
        Value::String(s) if s.contains("{{") => {
            Value::String(s.replace("{{SESSION_ID}}", sid).replace("{{CWD}}", cwd))
        }
        Value::Array(a) => Value::Array(a.iter().map(|x| substitute(x, sid, cwd)).collect()),
        Value::Object(o) => Value::Object(
            o.iter()
                .map(|(k, x)| (k.clone(), substitute(x, sid, cwd)))
                .collect(),
        ),
        other => other.clone(),
    }
}

fn emit(out: &mut impl Write, v: &Value) -> io::Result<()> {
    writeln!(out, "{v}")?;
    out.flush()
}

/// Entry point of `eludite-fake-claude`. Returns the process exit code.
pub fn main(args: Vec<String>) -> i32 {
    if args.iter().any(|a| a == "--version") {
        let v = std::env::var(VERSION_ENV).unwrap_or_else(|_| "2.1.287 (Claude Code)".into());
        println!("{v}");
        return 0;
    }
    let mut log = Log::open();
    let cwd = std::env::current_dir()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_default();
    let resume = arg_after(&args, "--resume");
    let sid = arg_after(&args, "--session-id")
        .or_else(|| resume.clone())
        .unwrap_or_default();
    let mcp = arg_after(&args, "--mcp-config")
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|s| serde_json::from_str::<Value>(&s).ok());
    let leaked: Vec<&str> = crate::discovery::CLAUDE_SESSION_ENV
        .iter()
        .copied()
        .filter(|v| std::env::var_os(v).is_some())
        .collect();
    log.write(json!({
        "event": "start",
        "exe": std::env::args().next(),
        "argv": args,
        "cwd": cwd,
        "leaked_session_env": leaked,
        "mcp_config": mcp,
        "path": std::env::var("PATH").unwrap_or_default(),
    }));
    let stdin = io::stdin();
    let mut input = stdin.lock();
    let mut out = io::stdout().lock();
    let result = if std::env::var(SCENARIO_ENV).as_deref() == Ok("stream") {
        stream(&mut input, &mut out, &mut log, &sid)
    } else {
        match std::env::var_os(FIXTURE_ENV) {
            Some(path) => {
                let segment = resume
                    .is_some()
                    .then(|| std::env::var(RESUME_ENV).unwrap_or_else(|_| "resume".into()));
                replay(
                    Path::new(&path),
                    segment.as_deref(),
                    &mut input,
                    &mut out,
                    &mut log,
                    &sid,
                    &cwd,
                )
            }
            None => {
                eprintln!("eludite-fake-claude: set {FIXTURE_ENV}");
                Ok(2)
            }
        }
    };
    match result {
        Ok(code) => code,
        Err(e) => {
            log.write(json!({"event": "io_error", "error": e.to_string()}));
            4
        }
    }
}

fn read_json(input: &mut impl BufRead, log: &mut Log) -> io::Result<Option<Value>> {
    let mut line = String::new();
    loop {
        line.clear();
        if input.read_line(&mut line)? == 0 {
            log.write(json!({"event": "eof"}));
            return Ok(None);
        }
        if line.trim().is_empty() {
            continue;
        }
        let v: Value = serde_json::from_str(line.trim()).map_err(io::Error::other)?;
        log.write(json!({"event": "input", "m": v}));
        return Ok(Some(v));
    }
}

/// The records of one recorded process: the first for a new session (`resume` `None`), else the one whose start
/// record has label `resume`; a fixture with no start records is one process.
fn segment(records: Vec<Value>, resume: Option<&str>) -> Vec<Value> {
    let mut segments: Vec<(Option<String>, Vec<Value>)> = vec![(None, Vec::new())];
    for r in records {
        if r["dir"] == "start" {
            let label = r["m"]["label"].as_str().map(str::to_owned);
            segments.push((label, Vec::new()));
        } else if let Some(last) = segments.last_mut() {
            last.1.push(r);
        }
    }
    let mut segments = segments
        .into_iter()
        .filter(|(label, recs)| label.is_some() || !recs.is_empty());
    match resume {
        None => segments.next().map(|s| s.1).unwrap_or_default(),
        Some(want) => segments
            .find(|(label, _)| label.as_deref() == Some(want))
            .map(|s| s.1)
            .unwrap_or_default(),
    }
}

fn replay(
    fixture: &Path,
    resume: Option<&str>,
    input: &mut impl BufRead,
    out: &mut impl Write,
    log: &mut Log,
    sid: &str,
    cwd: &str,
) -> io::Result<i32> {
    let lenient = std::env::var(LENIENT_ENV).as_deref() == Ok("1");
    let text = std::fs::read_to_string(fixture)?;
    // Recorded control request id -> the id the caller used.
    let mut ids: HashMap<String, String> = HashMap::new();
    let mut exit_code = 0;
    let records: Vec<Value> = text
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(serde_json::from_str)
        .collect::<Result<_, _>>()
        .map_err(io::Error::other)?;
    let records = segment(records, resume);
    let mut sent = vec![false; records.len()];
    let reply_id = |m: &Value| {
        (m["type"] == "control_response")
            .then(|| m.pointer("/response/request_id").and_then(Value::as_str))
            .flatten()
            .map(str::to_owned)
    };
    for i in 0..records.len() {
        let rec = &records[i];
        match rec["dir"].as_str() {
            Some("out") if !sent[i] => {
                sent[i] = true;
                emit_out(out, &rec["m"], sid, cwd, &ids)?;
            }
            Some("in") => {
                // The real CLI answers a control request as soon as it can,
                // even if the recording logged the caller's next message
                // first: send any reply already due before blocking on input.
                for j in i + 1..records.len() {
                    if records[j]["dir"] == "out"
                        && !sent[j]
                        && reply_id(&records[j]["m"]).is_some_and(|id| ids.contains_key(&id))
                    {
                        sent[j] = true;
                        emit_out(out, &records[j]["m"], sid, cwd, &ids)?;
                    }
                }
                let expected = substitute(&rec["m"], sid, cwd);
                let Some(got) = read_json(input, log)? else {
                    return Ok(0);
                };
                if let Some(why) = mismatch(&expected, &got) {
                    log.write(
                        json!({"event": "mismatch", "why": why, "expected": expected, "got": got}),
                    );
                    if !lenient {
                        return Ok(3);
                    }
                }
                if expected["type"] == "control_request"
                    && let (Some(rec_id), Some(id)) =
                        (expected["request_id"].as_str(), got["request_id"].as_str())
                {
                    ids.insert(rec_id.to_owned(), id.to_owned());
                }
            }
            Some("exit") => exit_code = rec["code"].as_i64().unwrap_or(0) as i32,
            Some("err") => {
                let line = substitute(&rec["m"]["line"], sid, cwd);
                eprintln!("{}", line.as_str().unwrap_or_default());
            }
            _ => {}
        }
    }
    // A recorded failure (a refused `--resume`) exits at once, as the real CLI did; otherwise, like the real CLI,
    // stay until stdin closes.
    if exit_code != 0 {
        return Ok(exit_code);
    }
    while read_json(input, log)?.is_some() {}
    Ok(exit_code)
}

fn emit_out(
    out: &mut impl Write,
    recorded: &Value,
    sid: &str,
    cwd: &str,
    ids: &HashMap<String, String>,
) -> io::Result<()> {
    let mut m = substitute(recorded, sid, cwd);
    if m["type"] == "control_response"
        && let Some(id) = m.pointer("/response/request_id").and_then(Value::as_str)
        && let Some(actual) = ids.get(id)
    {
        m["response"]["request_id"] = Value::String(actual.clone());
    }
    emit(out, &m)
}

fn mismatch(expected: &Value, got: &Value) -> Option<String> {
    if expected["type"] != got["type"] {
        return Some(format!("type {} != {}", got["type"], expected["type"]));
    }
    match expected["type"].as_str() {
        Some("control_request") => (expected["request"]["subtype"] != got["request"]["subtype"])
            .then(|| "control request subtype".to_owned()),
        Some("control_response") => {
            let (e, g) = (&expected["response"], &got["response"]);
            if e["request_id"] != g["request_id"] {
                Some("control response request_id".into())
            } else if e["response"]["behavior"] != g["response"]["behavior"] {
                Some(format!(
                    "behavior {} != {}",
                    g["response"]["behavior"], e["response"]["behavior"]
                ))
            } else {
                None
            }
        }
        _ => None,
    }
}

fn stream(
    input: &mut impl BufRead,
    out: &mut impl Write,
    log: &mut Log,
    sid: &str,
) -> io::Result<i32> {
    let chunks: usize = std::env::var("FAKE_CLAUDE_CHUNKS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(2000);
    let rate: f64 = std::env::var("FAKE_CLAUDE_RATE")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(200.);
    let text = "Lorem ipsum dolor sit amet, consectetur "; // 40 bytes
    while let Some(msg) = read_json(input, log)? {
        match msg["type"].as_str() {
            Some("control_request") => emit(
                out,
                &json!({"type": "control_response", "response": {"subtype": "success",
                    "request_id": msg["request_id"], "response": {"account": {"subscriptionType": "redacted"}}}}),
            )?,
            Some("user") => {
                emit(
                    out,
                    &json!({"type": "stream_event", "session_id": sid, "parent_tool_use_id": null,
                        "event": {"type": "message_start", "message": {"id": "msg_stream"}}}),
                )?;
                let start = Instant::now();
                for i in 0..chunks {
                    let due = start + Duration::from_secs_f64(i as f64 / rate);
                    if let Some(wait) = due.checked_duration_since(Instant::now()) {
                        std::thread::sleep(wait);
                    }
                    let t = if i % 8 == 7 { "line end.\n" } else { text };
                    emit(
                        out,
                        &json!({"type": "stream_event", "session_id": sid, "parent_tool_use_id": null,
                            "event": {"type": "content_block_delta", "index": 0,
                                "delta": {"type": "text_delta", "text": t}}}),
                    )?;
                }
                emit(
                    out,
                    &json!({"type": "result", "subtype": "success", "is_error": false,
                        "stop_reason": "end_turn", "session_id": sid}),
                )?;
            }
            _ => {}
        }
    }
    Ok(0)
}
