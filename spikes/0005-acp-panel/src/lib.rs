//! Brief 0005 spike: a throwaway GPUI panel hosting an ACP agent (Claude Code
//! through its ACP adapter), with Niello's MCP server exposing one command,
//! `diagnostics.list`, from an in-memory Error List.
//!
//! - `session`: everything off the UI thread. Spawns the agent (`niello-acp`),
//!   starts the MCP endpoint (`niello-mcp`), applies the permission policy and
//!   forwards events to the UI over a channel.
//! - `transcript`: the panel's model (rows for messages, tool calls, prompts).
//! - `panel`: the GPUI view.
//! - `bench`: frame-time and latency probes.
//!
//! Not production code: see docs/briefs/0005-report.md for what was learned.

pub mod bench;
pub mod panel;
pub mod session;
pub mod transcript;
