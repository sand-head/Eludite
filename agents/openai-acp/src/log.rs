//! Logging. stdout carries ACP only (Eludite's invariant 10), so logs go to stderr, or to the file named by
//! `ELUDITE_OPENAI_ACP_LOG`. Warnings always go out; informational lines (request bodies included) only when
//! `ELUDITE_OPENAI_ACP_LOG` is set (`stderr` or a path).
//!
//! The API key never reaches the log: every line passes through [`redact`] with the secrets registered by
//! [`add_secret`] (the key, at startup), so the key is replaced wherever it appears, and request headers are
//! logged without `Authorization` ([`crate::provider`]).

use std::fmt::Arguments;
use std::io::Write;
use std::sync::{Mutex, OnceLock, RwLock};

/// Environment variable enabling the verbose log: `stderr` or a file path.
pub const LOG_ENV: &str = "ELUDITE_OPENAI_ACP_LOG";

/// What a secret is replaced with.
pub const REDACTED: &str = "<redacted>";

struct Sink {
    verbose: bool,
    file: Option<Mutex<std::fs::File>>,
}

fn sink() -> &'static Sink {
    static SINK: OnceLock<Sink> = OnceLock::new();
    SINK.get_or_init(|| match std::env::var(LOG_ENV) {
        Ok(v) if v == "stderr" || v == "1" => Sink {
            verbose: true,
            file: None,
        },
        Ok(v) if !v.is_empty() => Sink {
            verbose: true,
            file: std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(v)
                .ok()
                .map(Mutex::new),
        },
        _ => Sink {
            verbose: false,
            file: None,
        },
    })
}

fn secrets() -> &'static RwLock<Vec<String>> {
    static SECRETS: OnceLock<RwLock<Vec<String>>> = OnceLock::new();
    SECRETS.get_or_init(RwLock::default)
}

/// Never write `secret` to the log: every later line has it replaced by [`REDACTED`]. Empty strings are ignored.
pub fn add_secret(secret: &str) {
    if secret.is_empty() {
        return;
    }
    let mut s = secrets().write().unwrap_or_else(|e| e.into_inner());
    if !s.iter().any(|x| x == secret) {
        s.push(secret.to_owned());
    }
}

/// `text` with every registered secret replaced.
pub fn redact(text: &str) -> String {
    let s = secrets().read().unwrap_or_else(|e| e.into_inner());
    let mut out = text.to_owned();
    for secret in s.iter() {
        if out.contains(secret.as_str()) {
            out = out.replace(secret.as_str(), REDACTED);
        }
    }
    out
}

/// Whether informational lines are written (so callers can skip building expensive ones).
pub fn verbose() -> bool {
    sink().verbose
}

fn write(level: &str, args: Arguments<'_>) {
    let s = sink();
    let line = redact(&format!("[eludite-openai-acp {level}] {args}\n"));
    if let Some(f) = &s.file
        && let Ok(mut f) = f.lock()
    {
        let _ = f.write_all(line.as_bytes());
        return;
    }
    let _ = std::io::stderr().lock().write_all(line.as_bytes());
}

pub fn info(args: Arguments<'_>) {
    if sink().verbose {
        write("info", args);
    }
}

pub fn warn(args: Arguments<'_>) {
    write("warn", args);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secrets_are_replaced_wherever_they_appear() {
        add_secret("sk-test-redact-1234");
        add_secret("");
        let line = redact(r#"{"key":"sk-test-redact-1234","x":"Bearer sk-test-redact-1234"}"#);
        assert!(!line.contains("sk-test-redact-1234"), "{line}");
        assert_eq!(line.matches(REDACTED).count(), 2);
        assert_eq!(redact("nothing here"), "nothing here");
    }
}
