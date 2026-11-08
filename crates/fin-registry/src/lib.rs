//! Plugin packages and registries (sub-project 8 spec §5–6, 8A plan Tasks 3–8, 11).
//!
//! - [`package`]: the strict `.finpkg` reader ([`package::unpack`]), the reproducible packer
//!   ([`package::pack`]) and tree hashes ([`package::tree_hash`], also used by `fin-node`'s
//!   build script for bundled plugins).
//! - [`tuf`]: the TUF client ([`tuf::TrustedRepo`]) and its per-root trust store.
//! - [`editor`]: [`editor::RepoBuilder`], which builds and signs registries (used by
//!   `fin-plugin` and the tests).
//! - [`registry`]: local-folder registries (config, kept trust, search, yanks, advisories);
//!   [`search`] reads only signed custom metadata.
//! - [`transport`]: where registry bytes come from (local folders; HTTP in test builds).
//! - [`plan`]: install plans ([`plan::make_plan`]) staged from a verified registry target
//!   ([`target::VerifiedTarget`]) or a bundled tree; [`store`]: commit, remove and the staging
//!   sweep. Nothing a package ships runs.
//! - [`clock`]: the plan and store clock (milliseconds); [`error`]: `{code, message, details,
//!   fix?}`.

pub mod advisory;
mod atomic;
pub mod clock;
pub mod editor;
pub mod error;
pub mod package;
pub mod plan;
mod private;
pub mod registry;
pub mod schema;
pub mod search;
pub mod store;
pub mod target;
pub mod transport;
pub mod tuf;

pub use clock::{Clock, FixedClock, SystemClock};
pub use error::RegistryError;
pub use store::Store;
