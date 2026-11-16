//! Command-line entry point for Iamb.

#[allow(clippy::print_stdout)] // The CLI's job is to write to stdout.
fn main() {
    println!("iamb {}", iamb_core::VERSION);
}

#[cfg(test)]
mod tests {
    #[test]
    fn version_is_available() {
        assert!(!iamb_core::VERSION.is_empty());
    }
}
