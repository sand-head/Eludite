use std::sync::Mutex;
use std::time::SystemTime;

use crate::{CommandId, PermissionClass};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Ok,
    Err(String),
}

/// One recorded invocation. Unknown commands are recorded with `permission: None`.
#[derive(Debug, Clone)]
pub struct AuditEntry {
    pub command: String,
    pub timestamp: SystemTime,
    pub permission: Option<PermissionClass>,
    pub outcome: Outcome,
}

impl AuditEntry {
    pub fn is_ok(&self) -> bool {
        self.outcome == Outcome::Ok
    }
}

/// Append-only, in-memory record of every invocation. Persisting it is future work.
#[derive(Debug, Default)]
pub struct AuditLog {
    entries: Mutex<Vec<AuditEntry>>,
}

impl AuditLog {
    pub fn record(&self, command: &str, permission: Option<PermissionClass>, outcome: Outcome) {
        let entry = AuditEntry {
            command: command.to_owned(),
            timestamp: SystemTime::now(),
            permission,
            outcome,
        };
        self.lock().push(entry);
    }

    /// A copy of all entries, oldest first.
    pub fn entries(&self) -> Vec<AuditEntry> {
        self.lock().clone()
    }

    pub fn len(&self) -> usize {
        self.lock().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Entries for one command.
    pub fn for_command(&self, id: &CommandId) -> Vec<AuditEntry> {
        self.lock()
            .iter()
            .filter(|e| e.command == id.as_str())
            .cloned()
            .collect()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Vec<AuditEntry>> {
        // A poisoned log is still a valid log; keep recording.
        self.entries.lock().unwrap_or_else(|e| e.into_inner())
    }
}
