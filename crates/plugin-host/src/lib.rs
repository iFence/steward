//! Plugin lifecycle management, routing, permissions and the IPC gateway.
//!
//! M2 wiring: [`route::RouteIndex`] routes launcher queries to the matching
//! plugin commands (command name / prefix / regex / dynamic, longest-prefix
//! priority) without waking any plugin process; [`host::PluginHost`] spawns
//! the `steward-plugin-runtime` binary (one shared pool plus one dedicated
//! process per `dedicated-process` plugin), invokes commands over NDJSON
//! JSON-RPC, and recycles crashed runtimes with exponential backoff.

pub mod host;
pub mod route;

pub use host::{HostConfig, HostEvent, PluginHost};
pub use route::{RouteHit, RouteIndex};

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
