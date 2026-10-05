//! `eludite-openai-acp`: ACP on stdin/stdout, an OpenAI-compatible server as the model.
//!
//! Usage:
//!   eludite-openai-acp --base-url URL [--model M] [--header NAME=VALUE]... [--tools core|all] [--catalog PATH|JSON]
//!                      [--max-completion-tokens N] [--chat-template-kwargs JSON] [--name NAME]
//!   eludite-openai-acp models --base-url URL [--header NAME=VALUE]... [--catalog PATH|JSON]
//!   eludite-openai-acp --version | --help
//!
//! Environment: ELUDITE_OPENAI_API_KEY (the key; none: no Authorization header), ELUDITE_OPENAI_ACP_LOG (`stderr`
//! or a file: verbose log, the key never in it). OPENAI_API_KEY is never read.

use std::process::ExitCode;

use eludite_openai_acp::agent::{self, API_KEY_ENV, Config};
use eludite_openai_acp::mcp::ToolsMode;
use eludite_openai_acp::models;

const HELP: &str =
    "eludite-openai-acp: Agent Client Protocol (stdio) agent over an OpenAI-compatible server.

Usage:
  eludite-openai-acp --base-url URL [--model M] [--header NAME=VALUE]... [--tools core|all]
                     [--catalog PATH|JSON] [--max-completion-tokens N] [--chat-template-kwargs JSON]
                     [--name NAME]
  eludite-openai-acp models --base-url URL [--header NAME=VALUE]... [--catalog PATH|JSON]
  eludite-openai-acp --version

URL is the API root ending in the version segment (http://localhost:8080/v1). The key is read
from $ELUDITE_OPENAI_API_KEY (never $OPENAI_API_KEY); without one no Authorization header is sent.
Its tools are the IDE's, from the MCP servers the ACP client passes. Logs go to stderr
($ELUDITE_OPENAI_ACP_LOG for verbose output, the key never in it); stdout carries ACP only.
`models` prints the server's models as JSON ({models, listing, message?}) and exits.";

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1).peekable();
    let listing = args.peek().map(String::as_str) == Some("models");
    if listing {
        args.next();
    }
    let mut config = Config {
        api_key: std::env::var(API_KEY_ENV).ok().filter(|k| !k.is_empty()),
        ..Config::default()
    };
    while let Some(arg) = args.next() {
        let mut value = |name: &str| args.next().ok_or_else(|| format!("{name} needs a value"));
        let r: Result<(), String> = (|| {
            match arg.as_str() {
                "--base-url" => config.base_url = value("--base-url")?,
                "--model" => config.model = Some(value("--model")?).filter(|m| !m.is_empty()),
                "--header" => {
                    let h = value("--header")?;
                    let (k, v) = h.split_once('=').ok_or("--header is NAME=VALUE")?;
                    config.headers.push((k.trim().to_owned(), v.to_owned()));
                }
                "--tools" => config.tools = value("--tools")?.parse::<ToolsMode>()?,
                "--catalog" => {
                    let c = value("--catalog")?;
                    let text = if c.trim_start().starts_with('[') {
                        c
                    } else {
                        std::fs::read_to_string(&c).map_err(|e| format!("--catalog {c}: {e}"))?
                    };
                    config.catalog = models::parse_catalog(&text)?;
                }
                "--max-completion-tokens" => {
                    config.limits.max_completion_tokens = Some(
                        value("--max-completion-tokens")?
                            .parse()
                            .map_err(|_| "--max-completion-tokens is a number")?,
                    )
                }
                "--chat-template-kwargs" => {
                    let v: serde_json::Value =
                        serde_json::from_str(&value("--chat-template-kwargs")?)
                            .map_err(|e| format!("--chat-template-kwargs is JSON: {e}"))?;
                    if !v.is_object() {
                        return Err("--chat-template-kwargs is a JSON object".into());
                    }
                    config.limits.chat_template_kwargs = Some(v);
                }
                "--name" => config.name = Some(value("--name")?),
                "--version" | "-V" => {
                    println!("eludite-openai-acp {}", env!("CARGO_PKG_VERSION"));
                    std::process::exit(0);
                }
                "--help" | "-h" => {
                    println!("{HELP}");
                    std::process::exit(0);
                }
                other => return Err(format!("unknown argument {other}")),
            }
            Ok(())
        })();
        if let Err(e) = r {
            return usage(&e);
        }
    }
    if !config.base_url.starts_with("http://") && !config.base_url.starts_with("https://") {
        return usage("--base-url is an http:// or https:// url");
    }
    if listing {
        let l = config.provider().list_models(&config.catalog);
        println!("{}", l.to_json());
        return ExitCode::SUCCESS;
    }
    match agent::run_stdio(config) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("eludite-openai-acp: {e:?}");
            ExitCode::FAILURE
        }
    }
}

fn usage(msg: &str) -> ExitCode {
    eprintln!("eludite-openai-acp: {msg}\n\n{HELP}");
    ExitCode::from(2)
}
