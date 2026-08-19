//! Standalone plugin host process.
//!
//! M2 milestone embeds QuickJS (`rquickjs`) here and speaks
//! `steward-ipc-protocol` over the platform IPC transport.

fn main() {
    println!(
        "steward-plugin-runtime {} (placeholder: QuickJS host arrives in M2)",
        env!("CARGO_PKG_VERSION")
    );
}
