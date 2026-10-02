//! Logging. stdout carries ACP only, so logs go to stderr, or to the file
//! named by `ELUDITE_CLAUDE_ACP_LOG`. Warnings always go out; informational
//! lines (including the child's stderr) only when `ELUDITE_CLAUDE_ACP_LOG` is
//! set (`stderr` or a path). The log never contains prompts, model output,
//! account details or credentials.

use std::fmt::Arguments;
use std::io::Write;
use std::sync::{Mutex, OnceLock};

/// Environment variable enabling the verbose log: `stderr` or a file path.
pub const LOG_ENV: &str = "ELUDITE_CLAUDE_ACP_LOG";

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

fn write(level: &str, args: Arguments<'_>) {
    let s = sink();
    let line = format!("[eludite-claude-acp {level}] {args}\n");
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
