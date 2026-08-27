//! Standalone plugin runtime process.
//!
//! Embeds QuickJS (`rquickjs`) and speaks `steward-ipc-protocol` over
//! newline-delimited JSON on stdin/stdout. The main process spawns one
//! instance as the shared isolate pool, or one instance per plugin with
//! `--dedicated` for privileged/heavy plugins (dedicated-process isolation).
//!
//! All diagnostics go to stderr; stdout carries only protocol frames so the
//! NDJSON stream stays parseable by the host.

use steward_plugin_runtime::{isolated_process, run_service, ServiceConfig};

fn main() -> anyhow::Result<()> {
    let dedicated = std::env::args().any(|arg| arg == "--dedicated");
    let config = if dedicated {
        isolated_process::dedicated_config()
    } else {
        ServiceConfig::default()
    };
    eprintln!(
        "steward-plugin-runtime {} ({})",
        env!("CARGO_PKG_VERSION"),
        if dedicated {
            "dedicated"
        } else {
            "shared-pool"
        }
    );
    run_service(&config)
}
