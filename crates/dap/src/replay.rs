//! The replaying adapter (brief 0033, feature `replay`): it serves a [`Connection`] that plays a recorded DAP session
//! ([`crate::record::Recording`]) back to a client, so the shell can be tested against what a real adapter said
//! without the adapter installed.
//!
//! - **Matching.** Each request the client sends is scrubbed by the recorder's rules (this run's paths and process
//!   id become the placeholders; [`crate::record::Scrubber`]) and matched to the first recorded request not yet
//!   matched with the same `command` and `arguments` (`seq` aside; a path's platform separators and `.exe` suffix
//!   aside). Thread, frame and variable ids need no mapping: the client only ever learns the recording's own ids,
//!   from the recorded answers. A request the recording did not see is answered `success: false` and recorded as a
//!   failure naming the nearest recorded request and the first differences ([`ReplayHandle::failures`]).
//! - **Order.** The adapter's messages are played in their recorded order, each once every request recorded before
//!   it has arrived: a response is never early, and the events that followed a response in the recording (`output`
//!   among them, in their order) follow it here. Each response's `request_seq` is the client's own `seq`.
//! - **Timing** is compressed to zero, or kept with [`ReplayOptions::real_time`].
//! - **The end.** When the recording ended with the adapter closing its output, the replayer closes the connection
//!   after its last message; otherwise when the client hangs up.
//!
//! [`ReplayHandle::waiting_for`] says what a stalled replay waits for (the recorded request the client has not
//! sent), for a test's timeout message.

use std::collections::HashMap;
use std::io::{BufReader, Write};
use std::path::PathBuf;
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::Connection;
use crate::framing;
use crate::record::{Ended, Recording, Scrubber, Substitution, differences, label};

/// How to replay.
#[derive(Debug, Clone, Default)]
pub struct ReplayOptions {
    /// (placeholder, this run's path) for `${ROOT}`, `${TMP}` and the others the recording uses.
    pub roots: Vec<(String, PathBuf)>,
    /// What `${PID}` becomes (a process id the test chooses).
    pub pid: i64,
    /// Keep the recorded gaps between the adapter's messages (`--real-time`); otherwise they are zero.
    pub real_time: bool,
}

enum Kind {
    Request {
        command: String,
        key: Value,
        seq: i64,
        /// The client's `seq` for it, once matched.
        matched: Option<i64>,
    },
    Adapter {
        message: Value,
    },
    /// The client's answers to reverse requests: not matched.
    Other,
}

struct Entry {
    t_ms: u64,
    kind: Kind,
}

struct State {
    entries: Vec<Entry>,
    /// The next entry to look at for an adapter message to play.
    cursor: usize,
    /// The first request entry not matched yet (`entries.len()` when all are).
    first_unmatched: usize,
    /// Recorded request `seq` → the client's `seq`.
    seqs: HashMap<i64, i64>,
    /// Answers to unmatched requests, to write at once.
    refusals: Vec<Value>,
    failures: Vec<String>,
    client_gone: bool,
    closed: bool,
    played: usize,
    /// The recorded time and the wall time of the last thing that happened (real time).
    last: (u64, Instant),
}

impl State {
    fn advance_unmatched(&mut self) {
        while self.first_unmatched < self.entries.len() {
            match &self.entries[self.first_unmatched].kind {
                Kind::Request { matched: None, .. } => break,
                _ => self.first_unmatched += 1,
            }
        }
    }
}

struct Shared {
    state: Mutex<State>,
    wake: Condvar,
    scrubber: Scrubber,
    substitution: Substitution,
    real_time: bool,
    ended: Option<Ended>,
    name: String,
}

/// Observes a replay from the test.
#[derive(Clone)]
pub struct ReplayHandle {
    shared: Arc<Shared>,
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl ReplayHandle {
    /// The requests that matched nothing, each with the nearest recorded request and the differences.
    pub fn failures(&self) -> Vec<String> {
        lock(&self.shared.state).failures.clone()
    }

    /// `Err` with every failure.
    pub fn check(&self) -> Result<(), String> {
        let f = self.failures();
        if f.is_empty() {
            Ok(())
        } else {
            Err(format!("replay of {}:\n{}", self.shared.name, f.join("\n")))
        }
    }

    /// How many adapter messages were played.
    pub fn played(&self) -> usize {
        lock(&self.shared.state).played
    }

    /// Whether every recorded message was played and every recorded request arrived.
    pub fn finished(&self) -> bool {
        let s = lock(&self.shared.state);
        s.cursor >= s.entries.len() && s.first_unmatched >= s.entries.len()
    }

    /// What the replay waits for: the next adapter message and the recorded request it waits on, or `None` when
    /// nothing is held back.
    pub fn waiting_for(&self) -> Option<String> {
        let s = lock(&self.shared.state);
        let next = (s.cursor..s.entries.len())
            .find(|&i| matches!(s.entries[i].kind, Kind::Adapter { .. }))?;
        if s.first_unmatched > next {
            return None;
        }
        let Kind::Adapter { message } = &s.entries[next].kind else {
            return None;
        };
        let Kind::Request { command, key, .. } = &s.entries[s.first_unmatched].kind else {
            return None;
        };
        Some(format!(
            "the recording's message #{} ({}) waits for the client's `{command}` {} (recorded as message #{}), which it has not sent",
            next + 1,
            label(message),
            key["arguments"],
            s.first_unmatched + 1
        ))
    }

    /// The recorded requests the client never sent, as `command arguments`.
    pub fn unsent(&self) -> Vec<String> {
        lock(&self.shared.state)
            .entries
            .iter()
            .filter_map(|e| match &e.kind {
                Kind::Request {
                    command,
                    key,
                    matched: None,
                    ..
                } => Some(format!("{command} {}", key["arguments"])),
                _ => None,
            })
            .collect()
    }
}

/// Strip what may differ by platform alone from a scrubbed path string: Windows' `.exe` suffix.
fn normalize_strings(v: &mut Value) {
    match v {
        Value::String(s) => {
            if s.starts_with("${")
                && let Some(t) = s.strip_suffix(".exe")
            {
                *s = t.to_owned();
            }
        }
        Value::Array(a) => a.iter_mut().for_each(normalize_strings),
        Value::Object(o) => o.values_mut().for_each(normalize_strings),
        _ => {}
    }
}

/// A request's matching key: `{ command, arguments }` with the strings normalized.
fn key_of(command: &str, arguments: Option<&Value>) -> Value {
    let mut arguments = arguments.cloned().unwrap_or(Value::Null);
    normalize_strings(&mut arguments);
    json!({ "command": command, "arguments": arguments })
}

/// Serve `recording` on an in-process connection. Requests are matched and messages played on the replayer's own
/// threads; the handle observes it.
pub fn serve(recording: &Recording, options: ReplayOptions) -> (Connection, ReplayHandle) {
    let entries: Vec<Entry> = recording
        .messages
        .iter()
        .map(|m| {
            let kind = match (m.dir, m.message["type"].as_str()) {
                (crate::record::Dir::Client, Some("request")) => {
                    let command = m.message["command"].as_str().unwrap_or_default().to_owned();
                    Kind::Request {
                        key: key_of(&command, m.message.get("arguments")),
                        command,
                        seq: m.message["seq"].as_i64().unwrap_or_default(),
                        matched: None,
                    }
                }
                (crate::record::Dir::Client, _) => Kind::Other,
                (crate::record::Dir::Adapter, _) => {
                    let mut message = m.message.clone();
                    if let Some(o) = message.as_object_mut() {
                        o.remove("truncated_by_recorder");
                    }
                    Kind::Adapter { message }
                }
            };
            Entry { t_ms: m.t_ms, kind }
        })
        .collect();
    let mut scrubber = Scrubber::new(&options.roots);
    scrubber.add_pid(options.pid);
    let mut state = State {
        entries,
        cursor: 0,
        first_unmatched: 0,
        seqs: HashMap::new(),
        refusals: Vec::new(),
        failures: Vec::new(),
        client_gone: false,
        closed: false,
        played: 0,
        last: (0, Instant::now()),
    };
    state.advance_unmatched();
    let name = format!("{} ({})", recording.adapter, recording.recorded_at);
    let shared = Arc::new(Shared {
        state: Mutex::new(state),
        wake: Condvar::new(),
        scrubber,
        substitution: Substitution {
            roots: options.roots.clone(),
            pid: options.pid,
        },
        real_time: options.real_time,
        ended: recording.ended,
        name: name.clone(),
    });
    let (client_reader, adapter_writer) = std::io::pipe().expect("pipe");
    let (adapter_reader, client_writer) = std::io::pipe().expect("pipe");
    let reader_shared = shared.clone();
    std::thread::Builder::new()
        .name("dap-replay-reader".into())
        .spawn(move || {
            let mut r = BufReader::new(adapter_reader);
            while let Ok(Some(body)) = framing::read_message(&mut r) {
                let Ok(msg) = serde_json::from_slice::<Value>(&body) else {
                    continue;
                };
                on_client(&reader_shared, msg);
            }
            lock(&reader_shared.state).client_gone = true;
            reader_shared.wake.notify_all();
        })
        .expect("spawn dap-replay-reader");
    let writer_shared = shared.clone();
    std::thread::Builder::new()
        .name("dap-replay".into())
        .spawn(move || play(&writer_shared, adapter_writer))
        .expect("spawn dap-replay");
    (
        Connection::from_streams(
            client_reader,
            client_writer,
            format!("replay of {name}: {}", recording.description),
        ),
        ReplayHandle { shared },
    )
}

/// A message from the client: match a request, or refuse it with the nearest recorded one.
fn on_client(shared: &Shared, msg: Value) {
    if msg["type"] != "request" {
        return;
    }
    let command = msg["command"].as_str().unwrap_or_default().to_owned();
    let seq = msg["seq"].as_i64().unwrap_or_default();
    let mut arguments = msg.get("arguments").cloned();
    if let Some(a) = arguments.as_mut() {
        shared.scrubber.scrub(a);
    }
    let key = key_of(&command, arguments.as_ref());
    let mut s = lock(&shared.state);
    let found = s
        .entries
        .iter()
        .position(|e| matches!(&e.kind, Kind::Request { key: k, matched: None, .. } if *k == key));
    match found {
        Some(i) => {
            let t = s.entries[i].t_ms;
            if let Kind::Request {
                matched, seq: rec, ..
            } = &mut s.entries[i].kind
            {
                *matched = Some(seq);
                let rec = *rec;
                s.seqs.insert(rec, seq);
            }
            if t >= s.last.0 {
                s.last = (t, Instant::now());
            }
            s.advance_unmatched();
        }
        None => {
            let failure = refusal_text(&s, &command, &key);
            s.failures.push(failure.clone());
            s.refusals.push(json!({
                "seq": 0,
                "type": "response",
                "request_seq": seq,
                "success": false,
                "command": command,
                "message": format!("replay: {failure}"),
            }));
        }
    }
    drop(s);
    shared.wake.notify_all();
}

/// Why `key` matched nothing: the nearest recorded request of that command (the next unmatched one first, then the
/// one with the fewest differences) and the differences, or the next recorded request when no request had that
/// command.
fn refusal_text(s: &State, command: &str, key: &Value) -> String {
    let candidates: Vec<(usize, &Value, bool)> = s
        .entries
        .iter()
        .enumerate()
        .filter_map(|(i, e)| match &e.kind {
            Kind::Request {
                command: c,
                key: k,
                matched,
                ..
            } if c == command => Some((i, k, matched.is_some())),
            _ => None,
        })
        .collect();
    let head = format!(
        "the client sent `{command}` {} which the recording did not see",
        key["arguments"]
    );
    let best = candidates
        .iter()
        .min_by_key(|(i, k, used)| (*used, differences(k, key, 100).len(), *i));
    match best {
        Some((i, k, used)) => {
            let diff = differences(&k["arguments"], &key["arguments"], 8);
            format!(
                "{head}; nearest recorded: message #{} ({}), differences (recorded, then sent):\n  {}",
                i + 1,
                if *used {
                    "already answered: the client sent it once more"
                } else {
                    "not sent yet"
                },
                if diff.is_empty() {
                    "none: it was answered already".to_owned()
                } else {
                    diff.join("\n  ")
                }
            )
        }
        None => {
            let next = s.entries[s.first_unmatched..]
                .iter()
                .find_map(|e| match &e.kind {
                    Kind::Request { command, key, .. } => {
                        Some(format!("`{command}` {}", key["arguments"]))
                    }
                    _ => None,
                })
                .unwrap_or_else(|| "nothing (every recorded request has arrived)".into());
            format!(
                "{head}; the recording has no `{command}` at all; the next request it expects is {next}"
            )
        }
    }
}

/// Whether the request entry `i` answers (when it is a response) has arrived.
fn answered_request_arrived(s: &State, i: usize) -> bool {
    let Kind::Adapter { message } = &s.entries[i].kind else {
        return true;
    };
    if message["type"] != "response" {
        return true;
    }
    let Some(rec) = message["request_seq"].as_i64() else {
        return true;
    };
    s.seqs.contains_key(&rec)
        || !s
            .entries
            .iter()
            .any(|e| matches!(&e.kind, Kind::Request { seq, .. } if *seq == rec))
}

/// Play the adapter's messages in order as their requests arrive, and the refusals at once.
fn play(shared: &Shared, mut writer: std::io::PipeWriter) {
    loop {
        let mut out: Vec<Value> = Vec::new();
        let mut delay = Duration::ZERO;
        {
            let mut s = lock(&shared.state);
            loop {
                out.append(&mut s.refusals);
                // The next adapter message, if its requests have all arrived.
                while s.cursor < s.entries.len()
                    && !matches!(s.entries[s.cursor].kind, Kind::Adapter { .. })
                {
                    s.cursor += 1;
                }
                // Every request recorded before it has arrived, and a response's own request too.
                let ready = s.cursor < s.entries.len()
                    && s.first_unmatched > s.cursor
                    && answered_request_arrived(&s, s.cursor);
                if ready {
                    let i = s.cursor;
                    let t = s.entries[i].t_ms;
                    if shared.real_time {
                        let (t0, w0) = s.last;
                        let target = w0 + Duration::from_millis(t.saturating_sub(t0));
                        delay = target.saturating_duration_since(Instant::now());
                        s.last = (t, target.max(Instant::now()));
                    }
                    let Kind::Adapter { message } = &s.entries[i].kind else {
                        unreachable!()
                    };
                    let mut m = message.clone();
                    if m["type"] == "response"
                        && let Some(rec) = m["request_seq"].as_i64()
                        && let Some(seq) = s.seqs.get(&rec)
                    {
                        m["request_seq"] = json!(*seq);
                    }
                    shared.substitution.apply(&mut m);
                    out.push(m);
                    s.cursor += 1;
                    s.played += 1;
                    if !shared.real_time {
                        continue;
                    }
                    break;
                }
                let done = s.cursor >= s.entries.len() && s.first_unmatched >= s.entries.len();
                if !out.is_empty() {
                    break;
                }
                if s.client_gone || (done && shared.ended == Some(Ended::Adapter)) {
                    s.closed = true;
                    return;
                }
                s = shared
                    .wake
                    .wait_timeout(s, Duration::from_millis(200))
                    .map(|(g, _)| g)
                    .unwrap_or_else(|e| e.into_inner().0);
            }
        }
        if !delay.is_zero() {
            std::thread::sleep(delay);
        }
        for m in out {
            let bytes = serde_json::to_vec(&m).expect("a message serializes");
            if framing::write_message(&mut writer, &bytes).is_err() {
                return;
            }
        }
        let _ = writer.flush();
    }
}
