//! Eludite extension SDK (PLAN.md D5, D6).
//!
//! MIT-licensed and free of GPL dependencies so extensions can be written under
//! any license. Today it only defines the extension manifest; the WASM component
//! interface will be added here when the extension host lands.

use serde::{Deserialize, Serialize};

/// An extension's manifest (`extension.toml` or `extension.json`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExtensionManifest {
    /// Unique extension name, e.g. `"csharp-snippets"`.
    pub name: String,
    /// Semver version string.
    pub version: String,
    /// Capabilities the extension requests, e.g. `"grammar"`, `"theme"`, `"command"`.
    #[serde(default)]
    pub capabilities: Vec<String>,
    /// Path to the entry module (a `.wasm` component), relative to the manifest.
    pub entry: String,
}

impl ExtensionManifest {
    /// Whether the manifest requests `capability`.
    pub fn requests(&self, capability: &str) -> bool {
        self.capabilities.iter().any(|c| c == capability)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deserialize_toml() {
        let m: ExtensionManifest = toml::from_str(
            r#"
            name = "vs-blue-theme"
            version = "1.0.0"
            capabilities = ["theme"]
            entry = "theme.wasm"
            "#,
        )
        .unwrap();
        assert_eq!(m.name, "vs-blue-theme");
        assert!(m.requests("theme"));
        assert!(!m.requests("command"));
    }

    #[test]
    fn deserialize_json_defaults_capabilities() {
        let m: ExtensionManifest =
            serde_json::from_str(r#"{"name":"x","version":"0.1.0","entry":"x.wasm"}"#).unwrap();
        assert!(m.capabilities.is_empty());
    }
}
