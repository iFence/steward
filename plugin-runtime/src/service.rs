//! NDJSON/JSON-RPC service loop.
//!
//! The runtime reads requests from stdin, dispatches them to the isolate
//! pool, and writes responses (and any toast notifications the plugins
//! emitted while handling the request) back to stdout. Every frame is one
//! newline-delimited JSON line; all diagnostics go to stderr so the protocol
//! stream stays clean.

use std::io::{BufRead, Write};

use anyhow::{Context as _, Result};
use serde::Deserialize;
use serde_json::{json, Value};
use steward_ipc_protocol::{
    code, decode_line, encode_line, method, Message, Request, Response, RpcError,
};
use steward_plugin_registry::PluginManifest;

use crate::isolate_pool::{
    InvokeError, IsolateId, IsolatePool, DEFAULT_HEAP_LIMIT, DEFAULT_MAX_STACK,
    DEFAULT_POOL_CAPACITY,
};

/// Service configuration for one runtime process.
#[derive(Debug, Clone)]
pub struct ServiceConfig {
    /// `--dedicated` mode: one plugin per process (dedicated-process
    /// isolation); otherwise the shared in-process pool.
    pub dedicated: bool,
    pub pool_capacity: usize,
    pub heap_limit: usize,
    pub max_stack: usize,
}

impl Default for ServiceConfig {
    fn default() -> Self {
        Self {
            dedicated: false,
            pool_capacity: DEFAULT_POOL_CAPACITY,
            heap_limit: DEFAULT_HEAP_LIMIT,
            max_stack: DEFAULT_MAX_STACK,
        }
    }
}

/// Parameters of `plugin.load`.
#[derive(Debug, Deserialize)]
struct LoadParams {
    /// Reverse-domain plugin id (informational; the manifest is the source of
    /// truth for commands and permissions).
    id: String,
    /// Absolute path of the bundled `dist/index.js` (esbuild IIFE output).
    entry_path: std::path::PathBuf,
    /// The validated plugin manifest.
    manifest: PluginManifest,
}

/// Parameters of `command.invoke`.
#[derive(Debug, Deserialize)]
struct CommandInvokeParams {
    isolate_id: IsolateId,
    command: String,
    /// Query text or arbitrary JSON payload for the command.
    #[serde(default)]
    input: Value,
    /// Execution deadline in milliseconds (host chooses 100 ms dynamic vs
    /// 500 ms static); the isolate is killed if it misses it.
    #[serde(default = "default_deadline")]
    deadline_ms: u64,
}

/// Parameters of `item.invoke`.
#[derive(Debug, Deserialize)]
struct ItemInvokeParams {
    isolate_id: IsolateId,
    item_id: String,
    #[serde(default = "default_deadline")]
    deadline_ms: u64,
}

/// Parameters of `plugin.unload`.
#[derive(Debug, Deserialize)]
struct UnloadParams {
    isolate_id: IsolateId,
}

fn default_deadline() -> u64 {
    500
}

/// Run the NDJSON service loop until stdin closes.
pub fn run_service(config: &ServiceConfig) -> Result<()> {
    let mut pool = IsolatePool::new(
        config.dedicated,
        config.pool_capacity,
        config.heap_limit,
        config.max_stack,
    );
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    let mut out = stdout.lock();

    for line in stdin.lock().lines() {
        let line = line.context("read request line")?;
        let Some(message) = decode_line(&line).context("decode request line")? else {
            continue;
        };
        let Message::Request(request) = message else {
            // M2 is strictly request/response from the host's perspective; the
            // runtime ignores inbound notifications and responses.
            continue;
        };

        let response = dispatch(&mut pool, &request);
        // Flush plugin-emitted notifications (toasts) before the response so
        // the host applies side effects in causal order.
        for notification in pool.drain_notifications() {
            write_line(&mut out, &Message::Notification(notification))?;
        }
        write_line(&mut out, &Message::Response(response))?;
    }
    Ok(())
}

fn dispatch(pool: &mut IsolatePool, request: &Request) -> Response {
    match request.method.as_str() {
        method::PLUGIN_LOAD => {
            let params = match parse_params::<LoadParams>(request) {
                Ok(params) => params,
                Err(response) => return response,
            };
            eprintln!("[runtime] loading plugin '{}'", params.id);
            match pool.load(&params.entry_path, &params.manifest) {
                Ok(isolate_id) => Response::ok(request.id, json!({ "isolate_id": isolate_id })),
                Err(error) => Response::error(
                    request.id,
                    RpcError::new(
                        code::INTERNAL_ERROR,
                        format!("failed to load plugin: {error:#}"),
                    ),
                ),
            }
        }
        method::COMMAND_INVOKE => {
            let params = match parse_params::<CommandInvokeParams>(request) {
                Ok(params) => params,
                Err(response) => return response,
            };
            match pool.invoke_command(
                params.isolate_id,
                &params.command,
                &params.input,
                params.deadline_ms,
            ) {
                Ok(view) => Response::ok(request.id, json!({ "view": view })),
                Err(error) => invoke_error(request.id, &params.command, error),
            }
        }
        method::ITEM_INVOKE => {
            let params = match parse_params::<ItemInvokeParams>(request) {
                Ok(params) => params,
                Err(response) => return response,
            };
            match pool.invoke_item(params.isolate_id, &params.item_id, params.deadline_ms) {
                Ok(()) => Response::ok(request.id, json!({})),
                Err(error) => invoke_error(request.id, "select", error),
            }
        }
        method::PLUGIN_UNLOAD => {
            let params = match parse_params::<UnloadParams>(request) {
                Ok(params) => params,
                Err(response) => return response,
            };
            pool.unload(params.isolate_id);
            Response::ok(request.id, json!({}))
        }
        method::PING => Response::ok(request.id, json!({ "pong": true })),
        _ => Response::error(
            request.id,
            RpcError::new(
                code::METHOD_NOT_FOUND,
                format!("unknown method '{}'", request.method),
            ),
        ),
    }
}

fn parse_params<T: for<'de> Deserialize<'de>>(
    request: &Request,
) -> std::result::Result<T, Response> {
    serde_json::from_value(request.params.clone()).map_err(|error| {
        Response::error(
            request.id,
            RpcError::new(
                code::INVALID_PARAMS,
                format!("invalid params for '{}': {error}", request.method),
            ),
        )
    })
}

fn invoke_error(id: u64, command: &str, error: InvokeError) -> Response {
    let (code, message) = match error {
        InvokeError::NotFound => (
            code::PLUGIN_NOT_FOUND,
            "plugin isolate is not loaded".to_string(),
        ),
        InvokeError::CommandNotFound => (
            code::COMMAND_NOT_FOUND,
            format!("command '{command}' is not registered by the plugin"),
        ),
        InvokeError::Timeout => (
            code::TIMEOUT,
            "plugin did not respond within its deadline; isolate killed".to_string(),
        ),
        InvokeError::Memory => (
            code::INTERNAL_ERROR,
            "plugin exceeded its heap limit; isolate killed".to_string(),
        ),
        InvokeError::PermissionDenied(message) => (code::PERMISSION_DENIED, message),
        InvokeError::Plugin(message) => (code::INTERNAL_ERROR, message),
        InvokeError::Internal(message) => (code::INTERNAL_ERROR, message),
    };
    Response::error(id, RpcError::new(code, message))
}

fn write_line(out: &mut impl Write, message: &Message) -> Result<()> {
    let line = encode_line(message).context("encode protocol message")?;
    out.write_all(line.as_bytes())
        .context("write protocol message")?;
    out.flush().context("flush protocol stream")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use steward_ipc_protocol::JSONRPC_VERSION;

    #[test]
    fn load_params_parse_from_json() {
        let params: LoadParams = serde_json::from_value(json!({
            "id": "com.example.calendar",
            "entry_path": "C:/plugins/calendar/dist/index.js",
            "manifest": {
                "id": "com.example.calendar",
                "name": "Calendar",
                "version": "1.0.0",
                "commands": [
                    { "name": "calendar", "title": "Calendar", "trigger": { "type": "command" } }
                ],
                "permissions": ["clipboard.write"],
                "isolation": "shared-pool"
            }
        }))
        .unwrap();
        assert_eq!(params.id, "com.example.calendar");
        assert_eq!(params.manifest.commands.len(), 1);
        assert_eq!(
            params.entry_path.to_string_lossy(),
            "C:/plugins/calendar/dist/index.js"
        );
    }

    #[test]
    fn unknown_method_returns_method_not_found() {
        let request = Request::new(1, "bogus.method", Value::Null);
        let response = dispatch(
            &mut IsolatePool::new(false, 1, DEFAULT_HEAP_LIMIT, DEFAULT_MAX_STACK),
            &request,
        );
        assert_eq!(response.error.unwrap().code, code::METHOD_NOT_FOUND);
        assert_eq!(response.jsonrpc, JSONRPC_VERSION);
    }

    #[test]
    fn ping_responds() {
        let request = Request::new_empty(7, method::PING);
        let response = dispatch(
            &mut IsolatePool::new(false, 1, DEFAULT_HEAP_LIMIT, DEFAULT_MAX_STACK),
            &request,
        );
        assert_eq!(response.result, Some(json!({ "pong": true })));
    }
}
