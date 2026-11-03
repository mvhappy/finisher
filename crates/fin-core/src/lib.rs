//! Finisher edit domain: model, operations, evaluation, branches.

pub mod error;
pub mod time;

pub use error::CoreError;
pub use time::{Rational, frame_duration, parse_time};

/// The crate version, as declared in its manifest.
pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_matches_manifest() {
        assert_eq!(version(), "0.0.0");
    }
}
