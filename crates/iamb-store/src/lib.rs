//! Local `SQLite` store, import and export.

#[cfg(test)]
mod tests {
    #[test]
    fn core_version_is_available() {
        assert!(!iamb_core::VERSION.is_empty());
    }
}
