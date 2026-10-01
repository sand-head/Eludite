//! Minimal JSON-RPC 2.0 message types.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The literal `"2.0"` version marker. Serializes as `"2.0"` and rejects anything else.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Version;

impl Serialize for Version {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str("2.0")
    }
}

impl<'de> Deserialize<'de> for Version {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let v = String::deserialize(d)?;
        if v == "2.0" {
            Ok(Version)
        } else {
            Err(serde::de::Error::custom(format!(
                "unsupported jsonrpc version {v:?}"
            )))
        }
    }
}

/// A request id: number or string.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Id {
    Number(i64),
    String(String),
}

impl From<i64> for Id {
    fn from(n: i64) -> Self {
        Id::Number(n)
    }
}

impl From<&str> for Id {
    fn from(s: &str) -> Self {
        Id::String(s.to_owned())
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Request {
    pub jsonrpc: Version,
    pub id: Id,
    pub method: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub params: Option<Value>,
}

impl Request {
    pub fn new(id: impl Into<Id>, method: impl Into<String>, params: Option<Value>) -> Self {
        Self {
            jsonrpc: Version,
            id: id.into(),
            method: method.into(),
            params,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Notification {
    pub jsonrpc: Version,
    pub method: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub params: Option<Value>,
}

impl Notification {
    pub fn new(method: impl Into<String>, params: Option<Value>) -> Self {
        Self {
            jsonrpc: Version,
            method: method.into(),
            params,
        }
    }
}

/// A response carries exactly one of `result` or `error`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Response {
    pub jsonrpc: Version,
    /// `None` only when the request id could not be determined (parse error).
    pub id: Option<Id>,
    #[serde(flatten)]
    pub payload: ResponsePayload,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ResponsePayload {
    Result(Value),
    Error(ErrorObject),
}

impl Response {
    pub fn ok(id: impl Into<Id>, result: Value) -> Self {
        Self {
            jsonrpc: Version,
            id: Some(id.into()),
            payload: ResponsePayload::Result(result),
        }
    }

    pub fn err(id: Option<Id>, error: ErrorObject) -> Self {
        Self {
            jsonrpc: Version,
            id,
            payload: ResponsePayload::Error(error),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ErrorObject {
    pub code: i64,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

impl ErrorObject {
    pub const PARSE_ERROR: i64 = -32700;
    pub const INVALID_REQUEST: i64 = -32600;
    pub const METHOD_NOT_FOUND: i64 = -32601;
    pub const INVALID_PARAMS: i64 = -32602;
    pub const INTERNAL_ERROR: i64 = -32603;

    pub fn new(code: i64, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            data: None,
        }
    }
}

/// Any JSON-RPC message. Variant order matters for untagged decoding:
/// requests have `id` + `method`, notifications only `method`, responses only `id`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Message {
    Request(Request),
    Notification(Notification),
    Response(Response),
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn request_round_trip() {
        let r = Request::new(1, "niello/ping", None);
        let s = serde_json::to_value(&r).unwrap();
        assert_eq!(
            s,
            json!({"jsonrpc": "2.0", "id": 1, "method": "niello/ping"})
        );
        assert_eq!(serde_json::from_value::<Request>(s).unwrap(), r);
    }

    #[test]
    fn response_round_trip() {
        let ok = Response::ok("a", json!({"pong": true}));
        let v = serde_json::to_value(&ok).unwrap();
        assert_eq!(
            v,
            json!({"jsonrpc": "2.0", "id": "a", "result": {"pong": true}})
        );
        assert_eq!(serde_json::from_value::<Response>(v).unwrap(), ok);

        let err = Response::err(
            Some(Id::Number(2)),
            ErrorObject::new(ErrorObject::METHOD_NOT_FOUND, "nope"),
        );
        let v = serde_json::to_value(&err).unwrap();
        assert_eq!(v["error"]["code"], json!(-32601));
        assert_eq!(serde_json::from_value::<Response>(v).unwrap(), err);
    }

    #[test]
    fn message_discriminates() {
        let m: Message = serde_json::from_value(json!({"jsonrpc":"2.0","method":"exit"})).unwrap();
        assert!(matches!(m, Message::Notification(_)));
        let m: Message =
            serde_json::from_value(json!({"jsonrpc":"2.0","id":3,"method":"shutdown"})).unwrap();
        assert!(matches!(m, Message::Request(_)));
        let m: Message =
            serde_json::from_value(json!({"jsonrpc":"2.0","id":3,"result":null})).unwrap();
        assert!(matches!(m, Message::Response(_)));
    }

    #[test]
    fn rejects_wrong_version() {
        let r = serde_json::from_value::<Request>(json!({"jsonrpc":"1.0","id":1,"method":"x"}));
        assert!(r.is_err());
    }
}
