//! Plugin packages and registries (sub-project 8 spec §5–6, 8A plan Tasks 3–8).
//!
//! - [`package`]: the strict `.finpkg` reader ([`package::unpack`]), the reproducible packer
//!   ([`package::pack`]) and tree hashes ([`package::tree_hash`], also used by `fin-node`'s
//!   build script for bundled plugins). It needs no TUF code, so a build-dependency with
//!   default features off stays small.
//! - [`plan`]: install plans ([`plan::make_plan`]) staged from a verified registry target
//!   ([`target::VerifiedTarget`]) or a bundled tree; [`store`]: commit, remove and the staging
//!   sweep. Nothing a package ships runs.
//! - [`clock`]: the injected clock; [`error`]: `{code, message, details, fix?}`.

pub mod clock;
pub mod error;
pub mod package;
pub mod plan;
pub mod schema;
pub mod store;
pub mod target;

pub use clock::{Clock, FixedClock, SystemClock};
pub use error::RegistryError;
pub use store::Store;
