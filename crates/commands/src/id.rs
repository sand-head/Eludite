use std::fmt;

use serde::{Deserialize, Serialize};

/// A command id: two or more dot-separated segments, each `[a-z][a-z0-9_]*`,
/// e.g. `eludite.file.open`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct CommandId(String);

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("invalid command id {0:?}: expected dotted lowercase segments like `eludite.file.open`")]
pub struct InvalidCommandId(pub String);

impl CommandId {
    pub fn new(id: impl Into<String>) -> Result<Self, InvalidCommandId> {
        let id = id.into();
        if Self::is_valid(&id) {
            Ok(Self(id))
        } else {
            Err(InvalidCommandId(id))
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    fn is_valid(id: &str) -> bool {
        let mut segments = 0;
        for segment in id.split('.') {
            segments += 1;
            let mut chars = segment.chars();
            match chars.next() {
                Some(c) if c.is_ascii_lowercase() => {}
                _ => return false,
            }
            if !chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_') {
                return false;
            }
        }
        segments >= 2
    }
}

impl TryFrom<String> for CommandId {
    type Error = InvalidCommandId;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl TryFrom<&str> for CommandId {
    type Error = InvalidCommandId;
    fn try_from(value: &str) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl From<CommandId> for String {
    fn from(id: CommandId) -> Self {
        id.0
    }
}

impl fmt::Display for CommandId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_ids() {
        for id in [
            "eludite.file.open",
            "a.b",
            "eludite.view.toggle_tool_window",
            "x1.y2",
        ] {
            assert!(CommandId::new(id).is_ok(), "{id}");
        }
    }

    #[test]
    fn invalid_ids() {
        for id in [
            "",
            "eludite",
            "Eludite.file",
            "eludite..open",
            ".a.b",
            "a.b.",
            "a.1b",
            "a.b-c",
            "a. b",
        ] {
            assert!(CommandId::new(id).is_err(), "{id}");
        }
    }

    #[test]
    fn serde_validates() {
        assert!(serde_json::from_str::<CommandId>("\"a.b\"").is_ok());
        assert!(serde_json::from_str::<CommandId>("\"A.b\"").is_err());
    }
}
