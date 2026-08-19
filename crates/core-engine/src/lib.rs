//! Search, indexing, fuzzy matching and ranking.
//!
//! No UI dependencies: everything here is unit-testable in isolation.
//! M1 milestone will add application scanning and `nucleo` integration.

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

#[cfg(test)]
mod tests {
    use super::VERSION;

    #[test]
    fn version_is_set() {
        assert!(!VERSION.is_empty());
    }
}
