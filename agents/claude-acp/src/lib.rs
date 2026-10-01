//! `niello-claude-acp`: an Agent Client Protocol (ACP) agent for Claude Code
//! that drives the user's installed `claude` executable directly over its
//! headless stream-json protocol. No Node, npm or npx is involved, and no
//! credential is read: the child inherits the user's own Claude Code login.
//!
//! ACP side: [`agent`] (protocol version 1 through the official
//! `agent-client-protocol` crate). Child side: [`process`] (one
//! `claude --print --input-format stream-json --output-format stream-json`
//! per session) and [`discovery`] (finding `claude`, checking its version).
//! The mapping between the two is pure and tested on recorded sessions:
//! [`translate`] (messages to `session/update`) and [`mapping`] (tool calls,
//! permissions, MCP config, prompt content).
//!
//! Public API boundary: the binary's command line and ACP on stdio. The
//! library exists for the binary and its tests; [`fake_claude`] is a
//! test-only replay of recorded `claude` sessions.

pub mod agent;
pub mod discovery;
pub mod fake_claude;
pub mod log;
pub mod mapping;
pub mod process;
pub mod translate;
