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
//!
//! Commands can be registered at any time from any thread. Each spec says whether agents see it
//! ([`CommandSpec::agent_visible`]); the caller of each invocation ([`Caller`], set by the MCP boundary with
//! [`with_caller`]) is recorded in the [`AuditLog`] with an agent's arguments, and the edits an agent's call produced
//! are joined to its entry when they are accepted or rejected. [`policy`] is the per-solution permission policy file
//! the MCP boundary applies to agents' calls (PLAN.md 5.3).

mod audit;
pub mod builtins;
mod caller;
pub mod diagnostics;
mod id;
pub mod policy;
mod registry;
pub mod view;
pub mod workspace;

pub use audit::{AuditEntry, AuditLog, EditRecord, EditState, Outcome};
pub use caller::{Caller, current_caller, next_call_id, with_caller};
pub use id::{CommandId, InvalidCommandId};
pub use registry::{CommandError, CommandRegistry, CommandSpec, Handler, PermissionClass};
