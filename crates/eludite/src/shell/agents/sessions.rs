//! The Agents window's session store (brief 0061): every conversation is kept in the person's per-workspace state,
//! never in the repository.
//!
//! - **Where.** `<workspace state dir>/agents/sessions/<id>.json` ([`dir_for`], over brief 0047's
//!   `workspace_state_dir`), the folder made with mode 0700 and each file written with mode 0600 on Unix, through a
//!   temporary file renamed over the old one, so a reader never sees half a record.
//! - **What.** A [`Record`]: `{"version": 1, "id", "agent", "acp_session_id", "title", "started", "last_activity",
//!   "model", "ended", "usage", "transcript"}`, the transcript being `Transcript::to_json`'s record (brief 0016's
//!   `--transcript-out`, as brief 0059 extended it). Times are RFC 3339 in UTC with milliseconds ([`rfc3339`]), so
//!   they sort as text.
//! - **How.** One `agents-sessions` thread does every read and write ([`SessionStore`]), in the order they were asked
//!   for: the record is serialized to a JSON value on the UI thread and written there; a folder is scanned for the
//!   history list ([`StoreEvent::Scanned`], the transcripts parsed and dropped) and a record loaded with its
//!   transcript rebuilt ([`StoreEvent::Loaded`]) there too. The [`KEEP`] most recent records by `last_activity` are
//!   kept: when a write makes more, the oldest files are deleted ([`StoreEvent::Pruned`]).
//!
//! Ids are UUID v4 ([`new_id`]); titles are the first prompt's first [`TITLE_CHARS`] characters on one line
//! ([`title_of`]); the history list says when a session last changed in words ([`relative`]).

use std::collections::HashMap;
use std::hash::{BuildHasher, Hasher};
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::transcript::Transcript;

/// Eludite's own id for a session (a UUID v4).
pub type SessionId = String;

/// The record's format version.
pub const RECORD_VERSION: u32 = 1;
/// How many records a workspace keeps (the most recent by `last_activity`).
pub const KEEP: usize = 100;
/// How many sessions may be live (their agent process running) at once.
pub const LIVE_LIMIT: usize = 8;
/// How many rows the history list shows.
pub const HISTORY_ROWS: usize = 50;
/// The title until the first prompt.
pub const NEW_TITLE: &str = "New session";
/// How much of the first prompt the title keeps.
pub const TITLE_CHARS: usize = 60;

/// `<workspace state dir>/agents/sessions` for workspace `root` under `state_root` (`<config dir>/eludite/workspaces`).
pub fn dir_for(state_root: &Path, root: &Path) -> PathBuf {
    crate::settings::workspace_state_dir(state_root, root)
        .join("agents")
        .join("sessions")
}

/// A new session id: 122 random bits as a UUID v4 (from the standard library's randomly keyed hasher, the clock and a
/// counter; an id, not a secret).
pub fn new_id() -> SessionId {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let word = |salt: u64| {
        let mut h = std::collections::hash_map::RandomState::new().build_hasher();
        h.write_u64(salt);
        h.write_u64(COUNTER.fetch_add(1, Ordering::Relaxed));
        h.write_u128(
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0, |d| d.as_nanos()),
        );
        h.write_u64(std::process::id().into());
        h.finish()
    };
    let (a, b) = (word(1), word(2));
    let mut bytes = [0u8; 16];
    bytes[..8].copy_from_slice(&a.to_be_bytes());
    bytes[8..].copy_from_slice(&b.to_be_bytes());
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let hex: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    )
}

/// Days since 1970-01-01 of a civil date, and back (Howard Hinnant's algorithms).
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (yoe + era * 400 + i64::from(m <= 2), m, d)
}

/// Milliseconds since the Unix epoch as RFC 3339 in UTC: `2026-10-05T21:02:03.004Z`.
pub fn rfc3339(ms: i64) -> String {
    let (days, rest) = (ms.div_euclid(86_400_000), ms.rem_euclid(86_400_000));
    let (y, m, d) = civil_from_days(days);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}.{:03}Z",
        rest / 3_600_000,
        rest / 60_000 % 60,
        rest / 1000 % 60,
        rest % 1000
    )
}

/// [`rfc3339`]'s text back to milliseconds since the epoch (UTC, `Z`; the fraction optional).
pub fn parse_rfc3339(s: &str) -> Option<i64> {
    let s = s.strip_suffix('Z')?;
    let (date, time) = s.split_once('T')?;
    let mut date = date.splitn(3, '-').map(|p| p.parse::<i64>().ok());
    let (y, m, d) = (date.next()??, date.next()??, date.next()??);
    let (hms, frac) = time.split_once('.').unwrap_or((time, "0"));
    let mut hms = hms.splitn(3, ':').map(|p| p.parse::<i64>().ok());
    let (h, mi, se) = (hms.next()??, hms.next()??, hms.next()??);
    let frac: String = frac.chars().chain("000".chars()).take(3).collect();
    let ms = frac.parse::<i64>().ok()?;
    Some((days_from_civil(y, m, d) * 86_400 + h * 3600 + mi * 60 + se) * 1000 + ms)
}

/// Now, in milliseconds since the epoch.
pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64)
}

/// Now as [`rfc3339`].
pub fn now() -> String {
    rfc3339(now_ms())
}

/// The local time's offset from UTC in minutes (from libgit2, which reads the system's time zone, as
/// `transcript::local_time` does).
pub fn local_offset_minutes() -> i64 {
    eludite_git::git2::Signature::now("eludite", "eludite@localhost")
        .map_or(0, |s| i64::from(s.when().offset_minutes()))
}

/// When `then` was, as the history list says it relative to `now` (both [`rfc3339`]) in a zone `offset` minutes from
/// UTC: `just now`, `5 min ago`, `3 h ago` (the same day), `yesterday`, else the local date `2026-10-03`.
pub fn relative(then: &str, now: &str, offset: i64) -> String {
    let (Some(t), Some(n)) = (parse_rfc3339(then), parse_rfc3339(now)) else {
        return then.to_owned();
    };
    let ago = (n - t).max(0) / 1000;
    let local_day = |ms: i64| (ms + offset * 60_000).div_euclid(86_400_000);
    let (day, today) = (local_day(t), local_day(n));
    if ago < 60 {
        "just now".into()
    } else if ago < 3600 {
        format!("{} min ago", ago / 60)
    } else if day == today {
        format!("{} h ago", ago / 3600)
    } else if day + 1 == today {
        "yesterday".into()
    } else {
        let (y, m, d) = civil_from_days(day);
        format!("{y:04}-{m:02}-{d:02}")
    }
}

/// A session's title from its first prompt: on one line (line breaks and other control characters as spaces, runs of
/// spaces as one), at most [`TITLE_CHARS`] characters.
pub fn title_of(prompt: &str) -> String {
    let line: String = prompt
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    let words: Vec<&str> = line.split_whitespace().collect();
    let title: String = words.join(" ").chars().take(TITLE_CHARS).collect();
    if title.is_empty() {
        NEW_TITLE.into()
    } else {
        title
    }
}

/// The record's usage: the session's last `usage_update` (the usage strip's numbers).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RecordUsage {
    pub used: u64,
    pub size: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost: Option<RecordCost>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RecordCost {
    pub amount: f64,
    pub currency: String,
}

/// What a session is, without its transcript: the history list's row.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionMeta {
    pub id: SessionId,
    pub agent: String,
    /// The agent's id for the session, once `session/new` or `session/load` answered.
    #[serde(default)]
    pub acp_session_id: Option<String>,
    pub title: String,
    pub started: String,
    pub last_activity: String,
    /// The model's name as the model picker showed it.
    #[serde(default)]
    pub model: Option<String>,
}

impl SessionMeta {
    pub fn new(agent: &str) -> Self {
        let now = now();
        Self {
            id: new_id(),
            agent: agent.to_owned(),
            acp_session_id: None,
            title: NEW_TITLE.into(),
            started: now.clone(),
            last_activity: now,
            model: None,
        }
    }

    /// Whether a prompt was sent (nothing is written before one).
    pub fn prompted(&self) -> bool {
        self.title != NEW_TITLE
    }
}

/// A session's record on disk.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Record {
    pub version: u32,
    #[serde(flatten)]
    pub meta: SessionMeta,
    /// The agent process stopped (or exited) when the record was written.
    #[serde(default)]
    pub ended: bool,
    #[serde(default)]
    pub usage: Option<RecordUsage>,
    /// `Transcript::to_json`'s record.
    #[serde(default)]
    pub transcript: Value,
}

/// [`SessionMeta`] and nothing else of a record (a scan skips the transcript).
#[derive(Deserialize)]
struct MetaOnly {
    version: u32,
    #[serde(flatten)]
    meta: SessionMeta,
}

/// What the store's thread reports.
pub enum StoreEvent {
    /// The records in `dir`, newest first.
    Scanned(PathBuf, Vec<SessionMeta>),
    /// Record `id` with its transcript rebuilt, or `None` when it is not there (or does not parse).
    Loaded(SessionId, Option<Box<(Record, Transcript)>>),
    /// Records deleted from `dir` to keep [`KEEP`].
    Pruned(PathBuf, Vec<SessionId>),
    /// A write failed (the message, for the log).
    Failed(String),
}

enum Job {
    Write(PathBuf, Box<Record>),
    Scan(PathBuf),
    Load(PathBuf, SessionId),
    #[cfg(test)]
    Flush(mpsc::Sender<()>),
}

/// The `agents-sessions` thread (see the module docs). Dropping the store ends it after the jobs queued.
pub struct SessionStore {
    tx: mpsc::Sender<Job>,
}

impl SessionStore {
    /// Start the thread; what it finds goes to `notify` (on that thread).
    pub fn start(notify: Arc<dyn Fn(StoreEvent) + Send + Sync>) -> Self {
        let (tx, rx) = mpsc::channel::<Job>();
        let spawned = std::thread::Builder::new()
            .name("agents-sessions".into())
            .spawn(move || {
                // Each folder's records and their `last_activity`, read once when the folder is first used.
                let mut indexes: HashMap<PathBuf, HashMap<SessionId, String>> = HashMap::new();
                while let Ok(job) = rx.recv() {
                    match job {
                        Job::Write(dir, record) => {
                            let index = indexes
                                .entry(dir.clone())
                                .or_insert_with(|| index_of(&scan(&dir)));
                            match write(&dir, &record) {
                                Ok(()) => {
                                    index.insert(
                                        record.meta.id.clone(),
                                        record.meta.last_activity.clone(),
                                    );
                                    let pruned = prune(&dir, index, &record.meta.id);
                                    if !pruned.is_empty() {
                                        notify(StoreEvent::Pruned(dir, pruned));
                                    }
                                }
                                Err(e) => notify(StoreEvent::Failed(format!(
                                    "{}: {e}",
                                    dir.join(format!("{}.json", record.meta.id)).display()
                                ))),
                            }
                        }
                        Job::Scan(dir) => {
                            let metas = scan(&dir);
                            indexes.insert(dir.clone(), index_of(&metas));
                            notify(StoreEvent::Scanned(dir, metas));
                        }
                        Job::Load(dir, id) => {
                            let loaded = load(&dir, &id).map(|r| {
                                let t = Transcript::from_json(&r.transcript);
                                Box::new((r, t))
                            });
                            notify(StoreEvent::Loaded(id, loaded));
                        }
                        #[cfg(test)]
                        Job::Flush(done) => {
                            let _ = done.send(());
                        }
                    }
                }
            });
        if let Err(e) = spawned {
            eprintln!("eludite: the agents' session store: {e}");
        }
        Self { tx }
    }

    /// Write `record` into `dir` (after the jobs queued before it).
    pub fn write(&self, dir: PathBuf, record: Record) {
        let _ = self.tx.send(Job::Write(dir, Box::new(record)));
    }

    /// List `dir`'s records ([`StoreEvent::Scanned`]).
    pub fn scan(&self, dir: PathBuf) {
        let _ = self.tx.send(Job::Scan(dir));
    }

    /// Load record `id` of `dir` with its transcript rebuilt ([`StoreEvent::Loaded`]).
    pub fn load(&self, dir: PathBuf, id: SessionId) {
        let _ = self.tx.send(Job::Load(dir, id));
    }

    /// Wait until every job queued so far is done (tests).
    #[cfg(test)]
    pub fn flush(&self) {
        let (tx, rx) = mpsc::channel();
        if self.tx.send(Job::Flush(tx)).is_ok() {
            let _ = rx.recv_timeout(std::time::Duration::from_secs(20));
        }
    }
}

fn index_of(metas: &[SessionMeta]) -> HashMap<SessionId, String> {
    metas
        .iter()
        .map(|m| (m.id.clone(), m.last_activity.clone()))
        .collect()
}

/// Whether `id` can name a record file (a UUID-like name: letters, digits and `-`).
fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// The records in `dir`, newest first (by `last_activity`); a file that does not parse, of another version, or
/// whose id is not its name is skipped.
pub fn scan(dir: &Path) -> Vec<SessionMeta> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut metas: Vec<SessionMeta> = entries
        .flatten()
        .filter_map(|e| {
            let path = e.path();
            let stem = path.file_stem()?.to_str()?.to_owned();
            if path.extension()? != "json" || !valid_id(&stem) {
                return None;
            }
            let text = std::fs::read_to_string(&path).ok()?;
            let m: MetaOnly = serde_json::from_str(&text).ok()?;
            (m.version == RECORD_VERSION && m.meta.id == stem).then_some(m.meta)
        })
        .collect();
    metas.sort_by(|a, b| b.last_activity.cmp(&a.last_activity));
    metas
}

/// Record `id` of `dir`.
pub fn load(dir: &Path, id: &str) -> Option<Record> {
    if !valid_id(id) {
        return None;
    }
    let text = std::fs::read_to_string(dir.join(format!("{id}.json"))).ok()?;
    let r: Record = serde_json::from_str(&text).ok()?;
    (r.version == RECORD_VERSION && r.meta.id == id).then_some(r)
}

/// Write `record` as `dir/<id>.json` (mode 0600, the folder 0700 on Unix) through a temporary file.
pub fn write(dir: &Path, record: &Record) -> std::io::Result<()> {
    if !valid_id(&record.meta.id) {
        return Err(std::io::Error::other("not a session id"));
    }
    if !dir.is_dir() {
        std::fs::create_dir_all(dir)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
        }
    }
    let text = serde_json::to_vec(record).map_err(std::io::Error::other)?;
    let path = dir.join(format!("{}.json", record.meta.id));
    let tmp = dir.join(format!(".{}.json.tmp", record.meta.id));
    {
        let mut opts = std::fs::OpenOptions::new();
        opts.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        let mut f = opts.open(&tmp)?;
        f.write_all(&text)?;
        f.flush()?;
    }
    std::fs::rename(&tmp, &path)
}

/// Delete the oldest records past [`KEEP`] (never `keep`, just written). Returns their ids.
fn prune(dir: &Path, index: &mut HashMap<SessionId, String>, keep: &str) -> Vec<SessionId> {
    if index.len() <= KEEP {
        return Vec::new();
    }
    let mut by_age: Vec<(String, SessionId)> = index
        .iter()
        .filter(|(id, _)| id.as_str() != keep)
        .map(|(id, at)| (at.clone(), id.clone()))
        .collect();
    by_age.sort();
    let extra = index.len() - KEEP;
    let mut pruned = Vec::new();
    for (_, id) in by_age.into_iter().take(extra) {
        let _ = std::fs::remove_file(dir.join(format!("{id}.json")));
        index.remove(&id);
        pruned.push(id);
    }
    pruned
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_uuid_v4_and_differ() {
        let a = new_id();
        let b = new_id();
        assert_ne!(a, b);
        assert_eq!(a.len(), 36);
        let parts: Vec<&str> = a.split('-').collect();
        assert_eq!(
            parts.iter().map(|p| p.len()).collect::<Vec<_>>(),
            [8, 4, 4, 4, 12]
        );
        assert!(parts[2].starts_with('4'));
        assert!(matches!(
            parts[3].chars().next(),
            Some('8' | '9' | 'a' | 'b')
        ));
        assert!(valid_id(&a));
        assert!(!valid_id("../x") && !valid_id("a/b") && !valid_id(""));
    }

    #[test]
    fn times_are_rfc3339_in_utc_and_read_back() {
        assert_eq!(rfc3339(0), "1970-01-01T00:00:00.000Z");
        let t = 1_791_234_123_004;
        let s = rfc3339(t);
        assert_eq!(s, "2026-10-05T21:02:03.004Z");
        assert_eq!(parse_rfc3339(&s), Some(t));
        assert_eq!(
            parse_rfc3339("2024-02-29T23:59:59Z"),
            Some(1_709_251_199_000)
        );
        assert_eq!(parse_rfc3339("2026-10-05 21:02"), None);
        // They sort as text.
        assert!(rfc3339(t) < rfc3339(t + 1));
    }

    #[test]
    fn the_history_says_when_in_words() {
        let now = "2026-10-05T21:00:00.000Z";
        assert_eq!(relative("2026-10-05T20:59:30.000Z", now, 0), "just now");
        assert_eq!(relative("2026-10-05T20:58:00.000Z", now, 0), "2 min ago");
        assert_eq!(relative("2026-10-05T18:00:00.000Z", now, 0), "3 h ago");
        assert_eq!(relative("2026-10-04T23:00:00.000Z", now, 0), "yesterday");
        assert_eq!(relative("2026-10-03T12:00:00.000Z", now, 0), "2026-10-03");
        // Central time (UTC-5 in October): 01:00 UTC on the 5th is the evening of the 4th.
        assert_eq!(relative("2026-10-05T01:00:00.000Z", now, -300), "yesterday");
        assert_eq!(
            relative("2026-10-03T12:00:00.000Z", now, -300),
            "2026-10-03"
        );
    }

    #[test]
    fn titles_are_the_first_prompt_on_one_line() {
        assert_eq!(title_of("Fix the\nfailing   test"), "Fix the failing test");
        let long = "x".repeat(80);
        assert_eq!(title_of(&long).chars().count(), TITLE_CHARS);
        assert_eq!(title_of("  \n "), NEW_TITLE);
    }

    fn record(id: &str, at: &str) -> Record {
        Record {
            version: RECORD_VERSION,
            meta: SessionMeta {
                id: id.into(),
                agent: "Fake agent".into(),
                acp_session_id: Some("acp-1".into()),
                title: format!("Session {id}"),
                started: at.into(),
                last_activity: at.into(),
                model: None,
            },
            ended: true,
            usage: None,
            transcript: serde_json::json!([{"user": "hi", "time": "14:02"}]),
        }
    }

    #[test]
    fn records_are_written_privately_scanned_newest_first_and_pruned_to_the_newest() {
        let dir = tempfile::tempdir().unwrap();
        let sessions = dir.path().join("agents/sessions");
        write(&sessions, &record("a", "2026-10-05T10:00:00.000Z")).unwrap();
        write(&sessions, &record("b", "2026-10-05T11:00:00.000Z")).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode(&sessions), 0o700);
            assert_eq!(mode(&sessions.join("a.json")), 0o600);
        }
        std::fs::write(sessions.join("junk.json"), "{").unwrap();
        let ids: Vec<String> = scan(&sessions).into_iter().map(|m| m.id).collect();
        assert_eq!(ids, ["b", "a"]);
        assert_eq!(
            load(&sessions, "a").unwrap(),
            record("a", "2026-10-05T10:00:00.000Z")
        );
        assert!(load(&sessions, "../a").is_none());
        // Past `KEEP`, the oldest go.
        let mut index = index_of(&scan(&sessions));
        for i in 0..KEEP {
            let id = format!("n{i:03}");
            let r = record(&id, &format!("2026-10-06T00:00:{:02}.{i:03}Z", i % 60));
            write(&sessions, &r).unwrap();
            index.insert(id.clone(), r.meta.last_activity.clone());
        }
        let pruned = prune(&sessions, &mut index, "n099");
        assert_eq!(pruned.len(), 2);
        assert!(pruned.contains(&"a".to_owned()) && pruned.contains(&"b".to_owned()));
        assert!(!sessions.join("a.json").exists());
        assert_eq!(scan(&sessions).len(), KEEP);
    }

    #[test]
    fn the_store_thread_writes_in_order_and_reports_what_it_finds() {
        let dir = tempfile::tempdir().unwrap();
        let sessions = dir.path().to_path_buf();
        let (tx, rx) = mpsc::channel();
        let tx = std::sync::Mutex::new(tx);
        let store = SessionStore::start(Arc::new(move |e| {
            let _ = tx.lock().unwrap().send(match e {
                StoreEvent::Scanned(_, m) => format!("scanned {}", m.len()),
                StoreEvent::Loaded(id, r) => format!("loaded {id} {}", r.is_some()),
                StoreEvent::Pruned(_, ids) => format!("pruned {}", ids.len()),
                StoreEvent::Failed(e) => format!("failed {e}"),
            });
        }));
        let mut r = record("s1", "2026-10-05T10:00:00.000Z");
        store.write(sessions.clone(), r.clone());
        r.meta.title = "Second".into();
        store.write(sessions.clone(), r);
        store.scan(sessions.clone());
        store.load(sessions.clone(), "s1".into());
        store.load(sessions.clone(), "missing".into());
        store.flush();
        let got: Vec<String> = rx.try_iter().collect();
        assert_eq!(got, ["scanned 1", "loaded s1 true", "loaded missing false"]);
        assert_eq!(load(&sessions, "s1").unwrap().meta.title, "Second");
    }
}
