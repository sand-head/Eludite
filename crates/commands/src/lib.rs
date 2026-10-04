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
//! the MCP boundary applies to agents' calls (PLAN.md 5.3). [`agents`] holds the Agents window's `eludite.agents.*`
//! commands and [`solution`] `eludite.solution.tree` (brief 0016). [`build`] holds the Build menu's `eludite.build.*`
//! commands and the Output window's `eludite.output.*` (brief 0017). [`debug`] holds the debugger's `eludite.debug.*`
//! commands (brief 0018), registered by the shell through a [`debug::DebugTarget`].
//! [`workspace_tree`] holds `eludite.workspace.tree`, every project of the open workspace whatever its build system
//! (brief 0019). [`settings`] holds the settings schema and `eludite.settings.*` with Tools > Options, and [`project`] the
//! Workspace context menu's Set as Startup Project and Open Containing Folder (brief 0020). [`browser`] holds the
//! browser automation commands `eludite.browser.*` (briefs 0023 and 0024), registered by the shell through a
//! [`browser::BrowserTarget`].
//!
//! [`test`] holds the Test Explorer's `eludite.test.*` commands (brief 0035), registered by the shell through a
//! [`test::TestCommands`]. [`git`] holds `eludite.git.*` (brief 0040), registered by the shell through a
//! [`git::GitCommands`], with the policy's `git` object applied by their escalation hooks. [`terminal`] holds the
//! integrated terminal's `eludite.terminal.*` (brief 0041), registered by the shell through a
//! [`terminal::TerminalCommands`], with the policy's `terminal` object applied by their escalation hooks.
//!
//! A command may register an escalation hook with its handler ([`CommandRegistry::register_with_escalation`],
//! ADR-0009): per call, from the input and a [`policy::PolicyView`], it raises the call's class above the spec's
//! (never lowers it) or refuses it for an agent. [`CommandRegistry::classify`] gives a call's [`CallClass`]; the MCP
//! boundary decides on it and invokes with it ([`CommandRegistry::invoke_as`]), and the audit entry records it.

pub mod agents;
mod audit;
pub mod browser;
pub mod build;
pub mod builtins;
mod caller;
pub mod debug;
pub mod diagnostics;
pub mod git;
mod id;
pub mod policy;
pub mod project;
mod registry;
pub mod settings;
pub mod solution;
pub mod terminal;
pub mod test;
pub mod view;
pub mod workspace;
pub mod workspace_tree;

pub use audit::{AuditEntry, AuditLog, EditRecord, EditState, Outcome};
pub use caller::{Caller, current_caller, next_call_id, with_caller};
pub use id::{CommandId, InvalidCommandId};
pub use registry::{
    CallClass, CommandError, CommandRegistry, CommandSpec, ESCALATES_KEY, Escalation,
    EscalationHook, Handler, PermissionClass,
};
