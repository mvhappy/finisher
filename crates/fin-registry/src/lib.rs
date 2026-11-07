//! Plugin packages and registries (sub-project 8 spec §6, 8A plan Tasks 3–8).
//!
//! - [`package`]: the strict `.finpkg` reader ([`package::unpack`]), the reproducible packer
//!   ([`package::pack`]) and tree hashes ([`package::tree_hash`], also used by `fin-node`'s
//!   build script for bundled plugins). It needs no TUF code, so a build-dependency with
//!   default features off stays small.

pub mod package;
