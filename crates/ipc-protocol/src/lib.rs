//! Message protocol shared between the main process and the plugin runtime.
//!
//! M2 milestone will grow this into the full JSON-RPC envelope (request /
//! response / notification) used over Unix Domain Sockets / Windows Named
//! Pipes. The minimal skeleton below is the serializable contract start.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// A JSON-RPC style request from the main process to the plugin runtime.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Request {
    pub id: u64,
    pub method: String,
    #[serde(default)]
    pub params: Value,
}

/// A JSON-RPC style response from the plugin runtime back to the main process.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Response {
    pub id: u64,
    pub result: Option<Value>,
    pub error: Option<Error>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Error {
    pub code: i64,
    pub message: String,
}

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_round_trips_through_json() {
        let request = Request {
            id: 42,
            method: "command.invoke".into(),
            params: serde_json::json!({ "name": "calculate" }),
        };
        let json = serde_json::to_string(&request).unwrap();
        let decoded: Request = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.id, 42);
        assert_eq!(decoded.method, "command.invoke");
    }
}
