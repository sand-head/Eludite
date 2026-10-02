//! Method-to-type mapping for typed requests and notifications.
//!
//! Each message in `protocol/schemas/host-rpc.md` that `eludite-protocol` types has a marker type implementing
//! [`RequestType`] or [`NotificationType`], so a client can write `client.request::<Completion>(params)` and get
//! the result type checked at compile time.

use serde::Serialize;
use serde::de::DeserializeOwned;

/// A JSON-RPC request with typed params and result.
pub trait RequestType {
    /// Wire method name.
    const METHOD: &'static str;
    /// True for forwarded LSP requests, which must carry `eluditeGeneration` (see host-rpc.md).
    const GENERATIONAL: bool;
    /// Params. `()` serializes as `null`, which a client sends as "no params".
    type Params: Serialize + DeserializeOwned;
    type Result: Serialize + DeserializeOwned;
}

/// A JSON-RPC notification with typed params.
pub trait NotificationType {
    const METHOD: &'static str;
    type Params: Serialize + DeserializeOwned;
}
