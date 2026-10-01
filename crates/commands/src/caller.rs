//! Who invoked a command: the user (menus, keys, clicks) or a hosted agent (through the MCP server or a tool call the
//! Agents window observed). The MCP boundary sets the caller for the duration of a call with [`with_caller`]; command
//! handlers read it with [`current_caller`] (the shell holds an agent's edits as pending changes), and the audit log
//! records it with every invocation.

use std::cell::RefCell;
use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};

/// The caller of a command invocation.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Caller {
    /// The UI: a menu item, a key, a click, or code in the shell.
    #[default]
    User,
    /// A hosted agent's tool call.
    Agent {
        /// The agent's name in the Agents window (`Claude Code`).
        agent: String,
        /// This tool call's id, unique in the process ([`next_call_id`]).
        call: u64,
        /// The agent's own id for the tool call (the ACP `toolCallId`), when known.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tool_call: Option<String>,
    },
}

impl Caller {
    pub fn is_agent(&self) -> bool {
        matches!(self, Caller::Agent { .. })
    }

    /// The agent tool call's id, for agent callers.
    pub fn call(&self) -> Option<u64> {
        match self {
            Caller::Agent { call, .. } => Some(*call),
            Caller::User => None,
        }
    }
}

thread_local! {
    static CALLER: RefCell<Caller> = const { RefCell::new(Caller::User) };
}

static NEXT_CALL: AtomicU64 = AtomicU64::new(1);

/// A fresh tool call id.
pub fn next_call_id() -> u64 {
    NEXT_CALL.fetch_add(1, Ordering::Relaxed)
}

/// Run `f` with `caller` as this thread's caller, then restore the previous one.
pub fn with_caller<R>(caller: Caller, f: impl FnOnce() -> R) -> R {
    struct Restore(Option<Caller>);
    impl Drop for Restore {
        fn drop(&mut self) {
            if let Some(c) = self.0.take() {
                CALLER.with(|cell| *cell.borrow_mut() = c);
            }
        }
    }
    let previous = CALLER.with(|cell| std::mem::replace(&mut *cell.borrow_mut(), caller));
    let _restore = Restore(Some(previous));
    f()
}

/// This thread's caller ([`Caller::User`] unless inside [`with_caller`]).
pub fn current_caller() -> Caller {
    CALLER.with(|cell| cell.borrow().clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn caller_is_scoped_and_restored() {
        assert_eq!(current_caller(), Caller::User);
        let agent = Caller::Agent {
            agent: "Fake".into(),
            call: next_call_id(),
            tool_call: None,
        };
        let seen = with_caller(agent.clone(), || {
            let inner = with_caller(Caller::User, current_caller);
            assert_eq!(inner, Caller::User);
            current_caller()
        });
        assert_eq!(seen, agent);
        assert!(seen.is_agent());
        assert_eq!(current_caller(), Caller::User);
        // Restored even when the closure panics.
        let _ = std::panic::catch_unwind(|| with_caller(agent.clone(), || panic!("boom")));
        assert_eq!(current_caller(), Caller::User);
        assert!(next_call_id() < next_call_id());
    }
}
