//! Steward plugin runtime: a standalone process that hosts QuickJS isolates.
//!
//! M2 ships two isolation grades (see `docs/architecture.md`):
//!
//! - [`isolate_pool`] hosts regular plugins in a shared pool of in-process
//!   QuickJS runtimes, each with a heap limit and a deadline-driven interrupt
//!   handler; a misbehaving isolate is dropped (killed) and recreated.
//! - [`isolated_process`] runs the same service in `--dedicated` mode: one
//!   plugin per process, so a crash or runaway resource use cannot touch any
//!   other plugin.
//!
//! [`service`] implements the NDJSON/JSON-RPC service loop on stdin/stdout.

pub mod isolate_pool;
pub mod isolated_process;
pub mod service;

pub use isolate_pool::{InvokeError, IsolateId, IsolatePool};
pub use service::{run_service, ServiceConfig};
