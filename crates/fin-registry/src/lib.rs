//! Plugin packages and registries (sub-project 8 spec §6, 8A plan Tasks 3–8).
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

pub mod advisory;
mod atomic;
pub mod editor;
pub mod package;
pub mod registry;
pub mod search;
pub mod transport;
pub mod tuf;
