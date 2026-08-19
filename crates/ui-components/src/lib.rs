//! Business-level UI components built on `gpui-component`.
//!
//! Kept dependency-free in M0 so the first build stays fast; the
//! `gpui-component` workspace dependency is added here once the first
//! component lands (M1+).

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

#[cfg(test)]
mod tests {
    use super::VERSION;

    #[test]
    fn version_is_set() {
        assert!(!VERSION.is_empty());
    }
}
