//! The command bus (PLAN.md 5.1, 5.3).
//!
//! Every action is a registered command with a dotted id, JSON schemas for
//! input and output, a permission class and an audit record. The UI invokes
//! commands through `CommandRegistry`; the MCP server (`niello-mcp`) exposes the
//! same specs to agents.

mod audit;
pub mod builtins;
mod id;
mod registry;

pub use audit::{AuditEntry, AuditLog, Outcome};
pub use id::{CommandId, InvalidCommandId};
pub use registry::{CommandError, CommandRegistry, CommandSpec, Handler, PermissionClass};
