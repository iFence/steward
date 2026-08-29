//! Message protocol shared between the main process and the plugin runtime.
//!
//! M2 carries JSON-RPC 2.0 envelopes over a newline-delimited JSON stream
//! (NDJSON) on the runtime process's stdin/stdout. The transport is swappable:
//! the Windows Named Pipe branch is scheduled for M4, and only the framing
//! helpers here (plus the transport-specific code in `plugin-host` /
//! `plugin-runtime`) would change.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// JSON-RPC protocol version marker, present on every message.
pub const JSONRPC_VERSION: &str = "2.0";

/// One entry of a plugin's clipboard-history snapshot. The host collects the
/// clipboard on a background thread and hands a recent slice to plugins whose
/// manifest grants `clipboard.history` (see `command.invoke` params).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ClipboardEntry {
    /// Stable id within a snapshot (the host assigns monotonically increasing
    /// row ids).
    pub id: String,
    /// The clipboard text.
    pub text: String,
    /// UNIX timestamp (seconds) when the text was copied.
    pub copied_at: i64,
}

/// A JSON-RPC 2.0 request from the main process to the plugin runtime.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Request {
    pub jsonrpc: String,
    pub id: u64,
    pub method: String,
    #[serde(default)]
    pub params: Value,
}

/// A JSON-RPC 2.0 response from the plugin runtime back to the main process.
/// Exactly one of `result` / `error` is present on a well-formed response.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Response {
    pub jsonrpc: String,
    pub id: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<RpcError>,
}

/// A JSON-RPC 2.0 notification: a fire-and-forget message with no id (e.g.
/// the runtime reporting a toast the plugin wants shown).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Notification {
    pub jsonrpc: String,
    pub method: String,
    #[serde(default)]
    pub params: Value,
}

/// One frame on the wire. `Notification` is never a response to a request.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(untagged)]
pub enum Message {
    Request(Request),
    Response(Response),
    Notification(Notification),
}

/// JSON-RPC error object.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RpcError {
    pub code: i64,
    pub message: String,
}

impl RpcError {
    pub fn new(code: i64, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

impl Request {
    pub fn new(id: u64, method: impl Into<String>, params: Value) -> Self {
        Self {
            jsonrpc: JSONRPC_VERSION.into(),
            id,
            method: method.into(),
            params,
        }
    }

    /// Convenience for requests without meaningful parameters.
    pub fn new_empty(id: u64, method: impl Into<String>) -> Self {
        Self::new(id, method, Value::Null)
    }
}

impl Response {
    pub fn ok(id: u64, result: Value) -> Self {
        Self {
            jsonrpc: JSONRPC_VERSION.into(),
            id,
            result: Some(result),
            error: None,
        }
    }

    pub fn error(id: u64, error: RpcError) -> Self {
        Self {
            jsonrpc: JSONRPC_VERSION.into(),
            id,
            result: None,
            error: Some(error),
        }
    }
}

impl Notification {
    pub fn new(method: impl Into<String>, params: Value) -> Self {
        Self {
            jsonrpc: JSONRPC_VERSION.into(),
            method: method.into(),
            params,
        }
    }
}

/// JSON-RPC standard error codes.
pub mod code {
    /// Invalid JSON was received by the server.
    pub const PARSE_ERROR: i64 = -32700;
    /// The JSON sent is not a valid Request object.
    pub const INVALID_REQUEST: i64 = -32600;
    /// The method does not exist / is not available.
    pub const METHOD_NOT_FOUND: i64 = -32601;
    /// Invalid method parameter(s).
    pub const INVALID_PARAMS: i64 = -32602;
    /// Internal JSON-RPC error.
    pub const INTERNAL_ERROR: i64 = -32603;

    // Steward-specific codes (application error range, -32000..-32099).
    /// The plugin called a host function its manifest did not grant.
    pub const PERMISSION_DENIED: i64 = -32000;
    /// The plugin did not answer within its deadline; its isolate was killed.
    pub const TIMEOUT: i64 = -32001;
    /// The requested plugin is not loaded in this runtime.
    pub const PLUGIN_NOT_FOUND: i64 = -32002;
    /// The requested command does not exist on the plugin.
    pub const COMMAND_NOT_FOUND: i64 = -32003;
    /// The manifest declared a permission M2 does not implement.
    pub const UNSUPPORTED_PERMISSION: i64 = -32004;
}

/// Main process -> runtime methods.
pub mod method {
    /// Load a plugin bundle into an isolate. Params: `{ id, entry_path,
    /// manifest }`; result: `{ isolate_id }`.
    pub const PLUGIN_LOAD: &str = "plugin.load";
    /// Run one plugin command. Params: `{ isolate_id, command, input,
    /// deadline_ms }`; result: `{ view }`.
    pub const COMMAND_INVOKE: &str = "command.invoke";
    /// Invoke a rendered list item's `onSelect`. Params: `{ isolate_id,
    /// item_id }`; result: `{}`.
    pub const ITEM_INVOKE: &str = "item.invoke";
    /// Invoke a view-level action (`ActionPanel`) on a plugin. Params:
    /// `{ isolate_id, action_id, item_id? }`; result: `{}`. The action's
    /// `onRun` is looked up by `action_id`; a missing `item_id` is passed as
    /// `undefined` to the plugin.
    pub const ACTION_INVOKE: &str = "action.invoke";
    /// Submit a rendered `form` view. Params: `{ isolate_id, values }`;
    /// result: `{}`. The plugin's `submit(values)` handler runs.
    pub const FORM_SUBMIT: &str = "form.submit";
    /// Stream results for a `search` view. Params: `{ isolate_id, query,
    /// deadline_ms }`; result: `{ view }`. The plugin's `search(query)`
    /// handler runs and returns a view (usually a `list` or `grid`) that
    /// replaces the search view's results area.
    pub const SEARCH_QUERY: &str = "search.query";
    /// Drop a plugin's isolate. Params: `{ isolate_id }`; result: `{}`.
    pub const PLUGIN_UNLOAD: &str = "plugin.unload";
    /// Liveness probe. Params: `{}`; result: `{ pong: true }`.
    pub const PING: &str = "ping";

    /// Runtime -> main process notification: show a toast. Params:
    /// `{ message, kind?, durationMs? }`.
    pub const TOAST_SHOW: &str = "toast.show";
    /// Runtime -> main process notification: open a URL in the user's default
    /// browser. Params: `{ url }`. Gated by the `open.url` permission.
    pub const OPEN_URL: &str = "open.url";
    /// Runtime -> main process notification: open a file path / shell target
    /// with the OS default handler (Windows `open` verb). Params: `{ path }`.
    /// Gated by the `open.path` permission.
    pub const OPEN_PATH: &str = "open.path";

    /// Runtime -> main process *request*: read a file on the host and return
    /// its contents. Params: `{ pending_id, plugin_id, path, encoding }`;
    /// result: `{ data, base64 }`. This is a runtime-initiated request that
    /// parks the plugin's promise until the host replies (cross-process await).
    pub const HOST_FS_READ: &str = "host.fs.read";
    /// Runtime -> main process *request*: write a file on the host. Params:
    /// `{ pending_id, plugin_id, path, data, base64 }`; result: `{}`. Sandboxed
    /// to the plugin's `fs_roots`; `base64` decodes `data` from base64 before
    /// writing (binary files).
    pub const HOST_FS_WRITE: &str = "host.fs.write";
    /// Runtime -> main process *request*: make an HTTP(S) request on the host.
    /// Params: `{ pending_id, plugin_id, method, url, headers, body,
    /// timeout_ms, max_bytes }`; result: `{ status, headers, body }`. Gated by
    /// the `network` permission; the host enforces http/https only.
    pub const HOST_NET_REQUEST: &str = "host.net.request";
}

/// Whether a method is a runtime-initiated request to the host (as opposed to
/// a host-initiated request to the runtime). Runtime->host requests use the
/// `host.` prefix and are answered on the runtime's stdin with a
/// [`Response`] carrying the same id.
pub fn is_host_request(method: &str) -> bool {
    method.starts_with("host.")
}

/// Encode one message as a single NDJSON line (including the trailing `\n`).
pub fn encode_line(message: &Message) -> serde_json::Result<String> {
    let mut line = serde_json::to_string(message)?;
    line.push('\n');
    Ok(line)
}

/// Decode one NDJSON line into a message. Blank lines are skipped.
pub fn decode_line(line: &str) -> serde_json::Result<Option<Message>> {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    serde_json::from_str(trimmed).map(Some)
}

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_round_trips_through_json() {
        let request = Request::new(
            42,
            method::COMMAND_INVOKE,
            serde_json::json!({ "isolate_id": 7, "command": "calendar" }),
        );
        let json = serde_json::to_string(&request).unwrap();
        let decoded: Request = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded, request);
        assert_eq!(decoded.jsonrpc, JSONRPC_VERSION);
        assert_eq!(decoded.id, 42);
    }

    #[test]
    fn response_error_round_trips() {
        let response = Response::error(
            42,
            RpcError::new(code::PERMISSION_DENIED, "permission not granted"),
        );
        let json = serde_json::to_string(&response).unwrap();
        let decoded: Response = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.error.unwrap().code, code::PERMISSION_DENIED);
        assert!(decoded.result.is_none());
    }

    #[test]
    fn message_envelope_round_trips_ndjson() {
        let messages = vec![
            Message::Request(Request::new_empty(1, method::PING)),
            Message::Response(Response::ok(1, serde_json::json!({ "pong": true }))),
            Message::Notification(Notification::new(
                method::TOAST_SHOW,
                serde_json::json!({ "message": "Copied" }),
            )),
        ];
        let mut wire = String::new();
        for message in &messages {
            wire.push_str(&encode_line(message).unwrap());
        }
        let decoded = wire
            .lines()
            .filter_map(|line| decode_line(line).unwrap())
            .collect::<Vec<_>>();
        assert_eq!(decoded, messages);
    }

    #[test]
    fn blank_lines_are_skipped() {
        assert_eq!(decode_line("").unwrap(), None);
        assert_eq!(decode_line("  \n").unwrap(), None);
    }

    #[test]
    fn clipboard_entry_round_trips() {
        let entry = ClipboardEntry {
            id: "7".into(),
            text: "hello".into(),
            copied_at: 1_752_000_000,
        };
        let json = serde_json::to_string(&entry).unwrap();
        let decoded: ClipboardEntry = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded, entry);
    }

    #[test]
    fn action_and_form_methods_are_distinct() {
        assert_ne!(method::ACTION_INVOKE, method::FORM_SUBMIT);
        assert_ne!(method::ACTION_INVOKE, method::ITEM_INVOKE);
        // The request params for a form submit carry a values object that
        // survives a JSON round trip.
        let request = Request::new(
            9,
            method::FORM_SUBMIT,
            serde_json::json!({ "isolate_id": 3, "values": { "name": "Ada" } }),
        );
        let json = serde_json::to_string(&request).unwrap();
        let decoded: Request = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.params["values"]["name"], "Ada");
    }
}
