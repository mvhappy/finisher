//! Finisher edit domain: model, operations, evaluation, branches.

pub mod error;
pub mod ids;
pub mod model;
pub mod schema;
pub mod time;
pub mod tx;

pub use error::CoreError;
pub use ids::{IdAlloc, IdKind, MemIds};
pub use model::*;
pub use schema::json_schema;
pub use tx::*;
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
