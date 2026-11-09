//! Media job params, inputs and results (spec §3.3, §4.2; plan 6A Task 11). Generated into
//! `@finisher/core-types`; the `media` worker family reads them, `fin-node` writes the inputs.
//!
//! - `in/snapshot.json` is a [`JobSnapshot`] (plan 6A Decision 2): the project at
//!   `(branch, rev)`, as `fin-node`'s `snapshot` returns it, with the branch beside it.
//! - `in/manifest.json` is a [`RenderManifest`] (spec §4.2): what each asset the snapshot
//!   references is on this machine, and the font bytes captions may use.
//! - `media.look` v1 takes [`LookParams`] and answers [`LookResult`]; each frame is also an
//!   `item` (`"0"`, `"1"`, … in `at` order) whose value is its [`LookFrame`].

//!
//! One module per job family's types: [`look`] (`media.look`), [`caps`]
//! (`media.capabilities`, v1 and the v2 report of 6B-1), [`manifest`] (the job inputs).

pub mod caps;
pub mod look;
pub mod manifest;

pub use caps::*;
pub use look::*;
pub use manifest::*;

use fin_core::CoreError;
use serde_json::json;

pub(crate) fn invalid(field: &str, message: String) -> CoreError {
    CoreError::new("JOB_PARAMS_INVALID", format!("params.{field}: {message}"))
        .with_details(json!({ "field": field }))
}
