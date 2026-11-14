//! The plugin model's pure half (sub-project 8 spec §4, 8A plan Task 2, 8B plan Task 2): the
//! manifest (`finisher-plugin.json`), closed permissions and the risks people see, permission
//! diffs, licence classes, the contribution kinds this host consumes, the Rust validation of a
//! manifest against the files of its package, the plugin protocol's types ([`protocol`]) and
//! the command argument schema subset ([`schema_subset`]).
//!
//! No I/O, clock, environment or randomness: package files come in through
//! [`validate::PackageFiles`], and the checks of contributed content (font catalogs, caption
//! styles) that need other crates come in through [`validate::ContentChecks`], which
//! `fin-node` implements over `fin-fonts` and `fin-core`. Of our crates it depends on
//! `fin-core` only (for the ops a command returns), and it builds for `wasm32-unknown-unknown`.

pub mod approval;
pub mod contribution;
pub mod licence;
pub mod manifest;
pub mod permission;
pub mod protocol;
pub mod risk;
pub mod schema;
pub mod schema_subset;
pub mod validate;

pub use approval::{Approval, Reapproval, reapproval_reasons, requires_reapproval};
pub use contribution::{ContributionKind, HOST_API, consumable, host_api};
pub use licence::{LicenceClass, classify};
pub use manifest::{
    CaptionStyleContribution, CommandContribution, Contributes, FontCatalogContribution,
    HookContribution, IdError, MANIFEST_FILE, Manifest, ModelRef, Platform, PluginId, PluginKind,
    Publisher, Runtime,
};
pub use permission::{
    FsAccess, FsScope, Origin, Permission, PermissionDiff, SecretMode, Widening, diff,
};
pub use risk::{Risk, overall, risks};
pub use schema_subset::{ArgProblem, SchemaSubset, validate_args};
pub use validate::{
    ContentChecks, Invalid, MAX_MANIFEST_BYTES, PackageFiles, SourceClass, StructuralChecks,
    ValidateCtx, validate,
};
