//! `eludite-claude-acp`: ACP on stdin/stdout, Claude Code as the agent.
//!
//! Usage:
//!   eludite-claude-acp [--claude PATH] [--model MODEL] [--effort LEVEL]   serve ACP on stdio
//!   eludite-claude-acp auth login                        run `claude auth login` (the ACP terminal login method)
//!   eludite-claude-acp --version | --help
//!
//! Environment: ELUDITE_CLAUDE_PATH (the `claude` executable), ELUDITE_CLAUDE_MODEL
//! (`--model` for new sessions), ELUDITE_CLAUDE_EFFORT (`--effort` for new sessions),
//! ELUDITE_CLAUDE_ACP_LOG (`stderr` or a file: verbose log).

use std::path::PathBuf;
use std::process::ExitCode;

use eludite_claude_acp::agent::{self, Config};
use eludite_claude_acp::discovery;

const HELP: &str = "eludite-claude-acp: Agent Client Protocol (stdio) adapter for Claude Code.

Usage:
  eludite-claude-acp [--claude PATH] [--model MODEL] [--effort LEVEL]
  eludite-claude-acp auth login
  eludite-claude-acp --version

Finds `claude` via --claude, $ELUDITE_CLAUDE_PATH, $PATH, then ~/.local/bin.
--model and --effort (low, medium, high, xhigh, max) are passed to `claude`
for every session; $ELUDITE_CLAUDE_MODEL and $ELUDITE_CLAUDE_EFFORT do the same,
else the client's choice in session/new's _meta.claudeCode.options.
Uses your existing Claude Code login. Logs go to stderr ($ELUDITE_CLAUDE_ACP_LOG
for verbose output); stdout carries ACP only.";

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    let mut config = Config::default();
    let first = args.next();
    if first.as_deref() == Some("auth") {
        return passthrough(std::iter::once("auth".to_owned()).chain(args).collect());
    }
    let mut next = first;
    while let Some(arg) = next {
        match arg.as_str() {
            "--claude" => match args.next() {
                Some(p) => config.claude = Some(PathBuf::from(p)),
                None => return usage("--claude needs a path"),
            },
            "--model" => match args.next() {
                Some(m) => config.model = Some(m),
                None => return usage("--model needs a value"),
            },
            "--effort" => match args.next() {
                Some(e) if agent::EFFORT_LEVELS.contains(&e.as_str()) => config.effort = Some(e),
                Some(e) => {
                    return usage(&format!(
                        "--effort takes low, medium, high, xhigh or max, not {e}"
                    ));
                }
                None => return usage("--effort needs a value"),
            },
            "--version" | "-V" => {
                println!("eludite-claude-acp {}", env!("CARGO_PKG_VERSION"));
                return ExitCode::SUCCESS;
            }
            "--help" | "-h" => {
                println!("{HELP}");
                return ExitCode::SUCCESS;
            }
            other => return usage(&format!("unknown argument {other}")),
        }
        next = args.next();
    }
    match agent::run_stdio(config) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("eludite-claude-acp: {e:?}");
            ExitCode::FAILURE
        }
    }
}

fn usage(msg: &str) -> ExitCode {
    eprintln!("eludite-claude-acp: {msg}\n\n{HELP}");
    ExitCode::from(2)
}

/// `eludite-claude-acp auth ...` runs `claude auth ...` in this terminal.
fn passthrough(args: Vec<String>) -> ExitCode {
    let claude = match discovery::discover_from_env(None) {
        Ok((p, _)) => p,
        Err(e) => {
            eprintln!("eludite-claude-acp: {e}");
            return ExitCode::FAILURE;
        }
    };
    let mut cmd = std::process::Command::new(claude);
    cmd.args(args);
    for var in discovery::CLAUDE_SESSION_ENV {
        cmd.env_remove(var);
    }
    match cmd.status() {
        Ok(s) if s.success() => ExitCode::SUCCESS,
        Ok(_) => ExitCode::FAILURE,
        Err(e) => {
            eprintln!("eludite-claude-acp: {e}");
            ExitCode::FAILURE
        }
    }
}
