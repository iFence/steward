//! Standalone plugin runtime process.
//!
//! Embeds QuickJS (`rquickjs`) and speaks `steward-ipc-protocol` over
//! newline-delimited JSON on stdin/stdout. The main process spawns one
//! instance as the shared isolate pool, or one instance per plugin with
//! `--dedicated` for privileged/heavy plugins (dedicated-process isolation).
//!
//! All diagnostics go to stderr; stdout carries only protocol frames so the
//! NDJSON stream stays parseable by the host.

// The app process is a GUI-subsystem binary (no console), so without this
// attribute Windows would allocate a *visible* console window for this
// process every time the host spawns it (each spawn is a black window on the
// user's desktop). The GUI subsystem still gets the piped stdin/stdout the
// host sets up; stderr simply goes nowhere when the parent has no console.
#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

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
