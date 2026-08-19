//! SQLite wrappers and configuration file access.
//!
//! M1 milestone will introduce `rusqlite` (bundled) here for index caches
//! and usage frequency.

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

#[cfg(test)]
mod tests {
    use super::VERSION;

    #[test]
    fn version_is_set() {
        assert!(!VERSION.is_empty());
    }
}
