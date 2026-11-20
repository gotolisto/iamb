//! Pure engine: data model, matcher, template language, date math. No I/O, no OS code.
//!
//! See `docs/architecture.md` §4 for how the pieces fit together.

pub mod datetime;
pub mod engine;
pub mod matcher;
pub mod model;
pub mod rich;
pub mod search;
pub mod sensitive;
pub mod template;

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
