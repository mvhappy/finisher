//! The plugin model's pure half (sub-project 8 spec §4, 8A plan Task 2): the manifest
//! (`finisher-plugin.json`), closed permissions and the risks people see, permission diffs,
//! licence classes, the contribution kinds this host consumes, and the Rust validation of a
//! manifest against the files of its package.
//!
//! No I/O, clock, environment or randomness: package files come in through
//! [`validate::PackageFiles`], and the checks of contributed content (font catalogs, caption
//! styles) that need other crates come in through [`validate::ContentChecks`], which
//! `fin-node` implements over `fin-fonts` and `fin-core`. This crate depends on no crate of
//! ours and builds for `wasm32-unknown-unknown`.

pub mod approval;
pub mod contribution;
pub mod licence;
pub mod manifest;
pub mod permission;
pub mod risk;
pub mod schema;
pub mod validate;

pub use approval::{Approval, Reapproval, reapproval_reasons, requires_reapproval};
pub use contribution::{ContributionKind, HOST_API, consumable, host_api};
pub use licence::{LicenceClass, classify};
pub use manifest::{
    CaptionStyleContribution, Contributes, FontCatalogContribution, IdError, MANIFEST_FILE,
    Manifest, ModelRef, Platform, PluginId, PluginKind, Publisher, Runtime,
};
pub use permission::{
    FsAccess, FsScope, Origin, Permission, PermissionDiff, SecretMode, Widening, diff,
};
pub use risk::{Risk, overall, risks};
pub use validate::{
    ContentChecks, Invalid, MAX_MANIFEST_BYTES, PackageFiles, SourceClass, StructuralChecks,
    ValidateCtx, validate,
};
