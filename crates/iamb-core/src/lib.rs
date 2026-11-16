//! Pure engine: data model, matcher, template language, date math. No I/O, no OS code.

/// Version of the `iamb-core` crate, taken from the workspace package version.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

#[cfg(test)]
mod tests {
    use super::VERSION;

    #[test]
    fn version_is_not_empty() {
        assert!(!VERSION.is_empty());
    }
}
