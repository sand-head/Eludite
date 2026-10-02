//! The command bus (PLAN.md 5.1, 5.3).
//!
//! Every action is a registered command with a dotted id, JSON schemas for
//! input and output, a permission class and an audit record. The UI invokes
//! commands through `CommandRegistry`; the MCP server (`eludite-mcp`) exposes the
//! same specs to agents. `diagnostics` holds `diagnostics.list`, the Error List
//! read command (brief 0005), registered separately because it needs a source.
//! `view` holds the `eludite.view.*` docking commands (brief 0008), registered
//! by whoever owns the layout through a [`view::ViewTarget`]. `workspace` holds
//! the solution, file and editor commands (brief 0012), registered by the shell
//! through a [`workspace::WorkspaceTarget`].

mod audit;
pub mod builtins;
pub mod diagnostics;
mod id;
mod registry;
pub mod view;
pub mod workspace;

pub use audit::{AuditEntry, AuditLog, Outcome};
pub use id::{CommandId, InvalidCommandId};
pub use registry::{CommandError, CommandRegistry, CommandSpec, Handler, PermissionClass};
