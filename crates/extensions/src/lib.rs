//! Extension host (PLAN.md D6, section 12 `crates/extensions`).
//!
//! Will load sandboxed WASM component extensions via wasmtime. wasmtime is not a
//! dependency yet to keep builds fast; for now this crate only parses manifests,
//! whose types live in the MIT `eludite-extension-sdk` crate.

use std::path::Path;

pub use eludite_extension_sdk::ExtensionManifest;

#[derive(Debug, thiserror::Error)]
pub enum ManifestError {
    #[error("unsupported manifest extension: {0}")]
    UnsupportedFormat(String),
    #[error("invalid TOML manifest: {0}")]
    Toml(#[from] toml::de::Error),
    #[error("invalid JSON manifest: {0}")]
    Json(#[from] serde_json::Error),
}

/// Parse a manifest, choosing TOML or JSON by the file name's extension.
pub fn parse_manifest(
    file_name: &Path,
    contents: &str,
) -> Result<ExtensionManifest, ManifestError> {
    match file_name.extension().and_then(|e| e.to_str()) {
        Some("toml") => Ok(toml::from_str(contents)?),
        Some("json") => Ok(serde_json::from_str(contents)?),
        other => Err(ManifestError::UnsupportedFormat(
            other.unwrap_or("").to_owned(),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_toml_and_json() {
        let toml_src =
            "name = \"a\"\nversion = \"1.0.0\"\ncapabilities = [\"command\"]\nentry = \"a.wasm\"\n";
        let json_src =
            r#"{"name":"a","version":"1.0.0","capabilities":["command"],"entry":"a.wasm"}"#;
        let a = parse_manifest(Path::new("extension.toml"), toml_src).unwrap();
        let b = parse_manifest(Path::new("extension.json"), json_src).unwrap();
        assert_eq!(a, b);
        assert_eq!(a.capabilities, vec!["command".to_string()]);
    }

    #[test]
    fn rejects_unknown_format() {
        assert!(matches!(
            parse_manifest(Path::new("extension.yaml"), ""),
            Err(ManifestError::UnsupportedFormat(_))
        ));
    }
}
