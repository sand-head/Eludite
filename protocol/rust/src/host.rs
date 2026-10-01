//! Params and results for the methods `niello-host` implements.
//!
//! This is the Rust mirror of `protocol/schemas/host-rpc.md`; field names are
//! camelCase on the wire.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// Method name constants.
pub mod methods {
    pub const INITIALIZE: &str = "initialize";
    pub const PING: &str = "niello/ping";
    pub const HOST_INFO: &str = "niello/host/info";
    pub const SHUTDOWN: &str = "shutdown";
    /// Notification, no response.
    pub const EXIT: &str = "exit";
}

/// The `hostName` value every conforming host reports.
pub const HOST_NAME: &str = "niello-host";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InitializeParams {
    pub client_name: String,
    pub client_version: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub solution_path: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InitializeResult {
    /// Always `"niello-host"`.
    pub host_name: String,
    pub host_version: String,
    #[serde(default)]
    pub capabilities: Map<String, Value>,
}

/// `niello/ping` takes no params.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PingResult {
    /// Always `true`.
    pub pong: bool,
    /// ISO-8601 timestamp produced by the host.
    pub timestamp: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DotnetSdk {
    pub version: String,
    pub path: String,
}

/// `niello/host/info` takes no params.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HostInfoResult {
    pub dotnet_sdks: Vec<DotnetSdk>,
    pub runtime: String,
    pub os: String,
}

/// `shutdown` takes no params and returns `null`.
pub type ShutdownResult = ();

#[cfg(test)]
mod tests {
    use super::*;
    use serde::de::DeserializeOwned;
    use serde_json::json;

    fn round_trip<T>(value: &T, expected: Value)
    where
        T: Serialize + DeserializeOwned + PartialEq + std::fmt::Debug,
    {
        let v = serde_json::to_value(value).unwrap();
        assert_eq!(v, expected);
        let back: T = serde_json::from_value(v).unwrap();
        assert_eq!(&back, value);
    }

    #[test]
    fn initialize() {
        round_trip(
            &InitializeParams {
                client_name: "niello".into(),
                client_version: "0.1.0".into(),
                solution_path: Some("/src/App.sln".into()),
            },
            json!({"clientName": "niello", "clientVersion": "0.1.0", "solutionPath": "/src/App.sln"}),
        );
        round_trip(
            &InitializeParams {
                client_name: "niello".into(),
                client_version: "0.1.0".into(),
                solution_path: None,
            },
            json!({"clientName": "niello", "clientVersion": "0.1.0"}),
        );
        let mut caps = Map::new();
        caps.insert("ping".into(), json!(true));
        round_trip(
            &InitializeResult {
                host_name: HOST_NAME.into(),
                host_version: "0.1.0".into(),
                capabilities: caps,
            },
            json!({"hostName": "niello-host", "hostVersion": "0.1.0", "capabilities": {"ping": true}}),
        );
    }

    #[test]
    fn ping() {
        round_trip(
            &PingResult {
                pong: true,
                timestamp: "2026-10-01T12:00:00Z".into(),
            },
            json!({"pong": true, "timestamp": "2026-10-01T12:00:00Z"}),
        );
    }

    #[test]
    fn host_info() {
        round_trip(
            &HostInfoResult {
                dotnet_sdks: vec![DotnetSdk {
                    version: "9.0.100".into(),
                    path: "/usr/share/dotnet/sdk".into(),
                }],
                runtime: ".NET 9.0.0".into(),
                os: "Linux".into(),
            },
            json!({
                "dotnetSdks": [{"version": "9.0.100", "path": "/usr/share/dotnet/sdk"}],
                "runtime": ".NET 9.0.0",
                "os": "Linux"
            }),
        );
    }

    #[test]
    fn shutdown_is_null() {
        round_trip(&(), Value::Null);
    }
}
