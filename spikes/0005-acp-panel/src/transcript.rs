//! The panel's model: a flat list of rows, so the GPUI `list` can virtualize
//! it and only re-measure the rows that changed. Agent text is split into one
//! row per line; while streaming, only the last line changes.

use std::collections::HashMap;

use eludite_acp::protocol::{
    PermissionOption, RequestPermissionRequest, SessionUpdate, ToolCall, ToolCallStatus,
};
use serde_json::{Value, json};

#[derive(Debug, Clone, PartialEq)]
pub enum PermissionState {
    Pending,
    Answered {
        allowed: bool,
        option: String,
    },
    /// Answered by policy without asking (class read).
    Auto {
        command: String,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct PermissionRow {
    pub key: u64,
    pub tool_call_id: String,
    pub title: String,
    pub arguments: Option<Value>,
    pub options: Vec<PermissionOption>,
    pub state: PermissionState,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Row {
    User(String),
    /// One line of agent text.
    Agent(String),
    Thought(String),
    Tool(ToolCall),
    Permission(PermissionRow),
    Notice(String),
    Error(String),
}

#[derive(Debug, Default)]
pub struct Transcript {
    pub rows: Vec<Row>,
    tools: HashMap<String, usize>,
    /// The last row is an agent line still receiving text.
    agent_open: bool,
    thought_open: bool,
    dirty_from: Option<usize>,
    synced_len: usize,
}

/// Rows `start..old_end` were replaced by rows `start..new_end`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Splice {
    pub start: usize,
    pub old_end: usize,
    pub new_end: usize,
}

impl Transcript {
    fn mark(&mut self, ix: usize) {
        self.dirty_from = Some(self.dirty_from.map_or(ix, |d| d.min(ix)));
    }

    fn push(&mut self, row: Row) {
        self.agent_open = false;
        self.thought_open = false;
        self.mark(self.rows.len());
        self.rows.push(row);
    }

    /// What changed since the last call, for `ListState::splice`.
    pub fn take_splice(&mut self) -> Option<Splice> {
        let start = self.dirty_from.take()?;
        let s = Splice {
            start,
            old_end: self.synced_len.max(start),
            new_end: self.rows.len(),
        };
        self.synced_len = self.rows.len();
        Some(s)
    }

    pub fn user(&mut self, text: &str) {
        self.push(Row::User(text.to_owned()));
    }

    pub fn notice(&mut self, text: impl Into<String>) {
        self.push(Row::Notice(text.into()));
    }

    pub fn error(&mut self, text: impl Into<String>) {
        self.push(Row::Error(text.into()));
    }

    pub fn agent_text(&mut self, text: &str) {
        let mut lines = text.split('\n');
        let first = lines.next().unwrap_or_default();
        if self.agent_open
            && let Some(Row::Agent(last)) = self.rows.last_mut()
        {
            last.push_str(first);
            self.mark(self.rows.len() - 1);
        } else {
            self.push(Row::Agent(first.to_owned()));
        }
        for line in lines {
            self.push(Row::Agent(line.to_owned()));
        }
        self.agent_open = true;
    }

    fn thought_text(&mut self, text: &str) {
        if self.thought_open
            && let Some(Row::Thought(last)) = self.rows.last_mut()
        {
            last.push_str(text);
            self.mark(self.rows.len() - 1);
        } else {
            self.push(Row::Thought(text.to_owned()));
            self.thought_open = true;
        }
    }

    pub fn apply(&mut self, update: &SessionUpdate) {
        match update {
            SessionUpdate::AgentMessageChunk(c) => {
                if let Some(t) = c.as_text() {
                    self.agent_text(t);
                }
            }
            SessionUpdate::AgentThoughtChunk(c) => {
                if let Some(t) = c.as_text() {
                    self.thought_text(t);
                }
            }
            SessionUpdate::UserMessageChunk(_) => {}
            SessionUpdate::ToolCall(t) => {
                if let Some(&ix) = self.tools.get(&t.tool_call_id) {
                    if let Row::Tool(existing) = &mut self.rows[ix] {
                        existing.apply(t.clone());
                    }
                    self.mark(ix);
                } else {
                    self.tools.insert(t.tool_call_id.clone(), self.rows.len());
                    self.push(Row::Tool(t.clone()));
                }
            }
            SessionUpdate::ToolCallUpdate(u) => {
                if let Some(&ix) = self.tools.get(&u.tool_call_id) {
                    if let Row::Tool(existing) = &mut self.rows[ix] {
                        existing.apply(u.clone());
                    }
                    self.mark(ix);
                }
            }
            SessionUpdate::Other { .. } => {}
        }
    }

    /// A permission request, asked (`auto: None`) or auto-answered by policy.
    pub fn permission(&mut self, key: u64, req: &RequestPermissionRequest, auto: Option<String>) {
        let tc = &req.tool_call;
        let title = tc
            .agent_tool_name()
            .map(str::to_owned)
            .or_else(|| tc.title.clone())
            .unwrap_or_else(|| tc.tool_call_id.clone());
        self.push(Row::Permission(PermissionRow {
            key,
            tool_call_id: tc.tool_call_id.clone(),
            title,
            arguments: tc.raw_input.clone(),
            options: req.options.clone(),
            state: match auto {
                Some(command) => PermissionState::Auto { command },
                None => PermissionState::Pending,
            },
        }));
    }

    pub fn answer_permission(&mut self, key: u64, allowed: bool, option: &str) -> bool {
        for (ix, row) in self.rows.iter_mut().enumerate() {
            if let Row::Permission(p) = row
                && p.key == key
                && p.state == PermissionState::Pending
            {
                p.state = PermissionState::Answered {
                    allowed,
                    option: option.to_owned(),
                };
                self.mark(ix);
                return true;
            }
        }
        false
    }

    pub fn pending_permissions(&self) -> Vec<u64> {
        self.rows
            .iter()
            .filter_map(|r| match r {
                Row::Permission(p) if p.state == PermissionState::Pending => Some(p.key),
                _ => None,
            })
            .collect()
    }

    pub fn tool(&self, tool_call_id: &str) -> Option<&ToolCall> {
        match self.rows.get(*self.tools.get(tool_call_id)?) {
            Some(Row::Tool(t)) => Some(t),
            _ => None,
        }
    }

    pub fn tools(&self) -> impl Iterator<Item = &ToolCall> {
        self.rows.iter().filter_map(|r| match r {
            Row::Tool(t) => Some(t),
            _ => None,
        })
    }

    /// All agent text, lines joined with `\n`.
    pub fn agent_message(&self) -> String {
        let mut out = String::new();
        let mut prev_agent = false;
        for r in &self.rows {
            if let Row::Agent(t) = r {
                if prev_agent {
                    out.push('\n');
                }
                out.push_str(t);
                prev_agent = true;
            } else {
                if prev_agent {
                    out.push('\n');
                }
                prev_agent = false;
            }
        }
        out
    }

    /// The transcript as JSON, for the report.
    pub fn to_json(&self) -> Value {
        let mut out = Vec::new();
        let mut agent = String::new();
        let flush = |agent: &mut String, out: &mut Vec<Value>| {
            if !agent.is_empty() {
                out.push(json!({"agent": std::mem::take(agent)}));
            }
        };
        for r in &self.rows {
            match r {
                Row::Agent(t) => {
                    if !agent.is_empty() {
                        agent.push('\n');
                    }
                    agent.push_str(t);
                    continue;
                }
                _ => flush(&mut agent, &mut out),
            }
            out.push(match r {
                Row::User(t) => json!({"user": t}),
                Row::Thought(t) => json!({"thought": t}),
                Row::Tool(t) => json!({"tool_call": {
                    "id": t.tool_call_id, "tool": t.agent_tool_name(), "title": t.title, "kind": t.kind,
                    "status": t.status, "arguments": t.raw_input, "result": t.content_text()
                }}),
                Row::Permission(p) => json!({"permission": {
                    "tool": p.title, "arguments": p.arguments,
                    "options": p.options.iter().map(|o| &o.name).collect::<Vec<_>>(),
                    "state": format!("{:?}", p.state)
                }}),
                Row::Notice(t) => json!({"notice": t}),
                Row::Error(t) => json!({"error": t}),
                Row::Agent(_) => unreachable!(),
            });
        }
        flush(&mut agent, &mut out);
        Value::Array(out)
    }
}

pub fn status_label(s: Option<ToolCallStatus>) -> &'static str {
    match s {
        None | Some(ToolCallStatus::Pending) => "pending",
        Some(ToolCallStatus::InProgress) => "running",
        Some(ToolCallStatus::Completed) => "completed",
        Some(ToolCallStatus::Failed) => "failed",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use eludite_acp::protocol::ContentBlock;

    #[test]
    fn agent_lines_split_and_only_tail_is_dirty() {
        let mut t = Transcript::default();
        t.user("hi");
        assert_eq!(
            t.take_splice(),
            Some(Splice {
                start: 0,
                old_end: 0,
                new_end: 1
            })
        );
        t.apply(&SessionUpdate::AgentMessageChunk(ContentBlock::text(
            "Hello ",
        )));
        t.apply(&SessionUpdate::AgentMessageChunk(ContentBlock::text(
            "world\nsecond",
        )));
        assert_eq!(
            t.take_splice(),
            Some(Splice {
                start: 1,
                old_end: 1,
                new_end: 3
            })
        );
        t.apply(&SessionUpdate::AgentMessageChunk(ContentBlock::text(
            " line",
        )));
        // Only the last row changed.
        assert_eq!(
            t.take_splice(),
            Some(Splice {
                start: 2,
                old_end: 3,
                new_end: 3
            })
        );
        assert_eq!(t.take_splice(), None);
        assert_eq!(t.agent_message(), "Hello world\nsecond line");
    }

    #[test]
    fn tool_updates_fold_into_one_row() {
        let mut t = Transcript::default();
        let call = ToolCall {
            tool_call_id: "a".into(),
            title: Some("x".into()),
            status: Some(ToolCallStatus::Pending),
            ..Default::default()
        };
        t.apply(&SessionUpdate::ToolCall(call));
        t.apply(&SessionUpdate::AgentMessageChunk(ContentBlock::text(
            "after",
        )));
        t.take_splice();
        t.apply(&SessionUpdate::ToolCallUpdate(ToolCall {
            tool_call_id: "a".into(),
            status: Some(ToolCallStatus::Completed),
            ..Default::default()
        }));
        assert_eq!(
            t.take_splice(),
            Some(Splice {
                start: 0,
                old_end: 2,
                new_end: 2
            })
        );
        assert_eq!(t.tool("a").unwrap().status, Some(ToolCallStatus::Completed));
        assert_eq!(t.tool("a").unwrap().title.as_deref(), Some("x"));
    }
}
