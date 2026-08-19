//! Plugin lifecycle management, routing, permissions and the IPC gateway.
//!
//! M2 milestone will implement trigger routing and the minimal-permission
//! model on top of `steward-ipc-protocol`.

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

#[cfg(test)]
mod tests {
    use super::VERSION;

    #[test]
    fn version_is_set() {
        assert!(!VERSION.is_empty());
    }
}
