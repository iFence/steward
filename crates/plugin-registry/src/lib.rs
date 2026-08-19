//! Plugin metadata cache (SQLite): incremental scanning / indexing.
//!
//! M2 milestone will cache parsed manifests so cold start never does a full
//! filesystem scan.

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

#[cfg(test)]
mod tests {
    use super::VERSION;

    #[test]
    fn version_is_set() {
        assert!(!VERSION.is_empty());
    }
}
