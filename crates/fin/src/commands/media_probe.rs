//! `fin media probe <asset>` (plan 6B-1 Task 11): the decode-level probe of one asset by a
//! `media.probeDeep` job on the server's media worker (`media.probeDeep`). `fin` links no media
//! crate: it sends the request and prints the answer, `{asset, fingerprint, codec, decodable,
//! backend?, class?, hw?, hdr?, hdrSource?, firstFrame?, failure?}`. A codec this machine
//! cannot decode is `decodable: false` with the decoder's `failure` (exit 0); `hdr` is the
//! container's HDR metadata, else the H.264/HEVC SEI's (`hdrSource`). Nothing is committed.
//! `--branch` picks the branch whose head names the asset.

use clap::Args;
use fin_core::CoreError;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::{Globals, target_branch};
use crate::backend::Backend;
use crate::output::unavailable;

/// Decode the first frame of an asset and read its in-band HDR metadata.
#[derive(Debug, Clone, Args, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProbeArgs {
    /// The asset's id.
    pub asset: String,
}

pub fn run(backend: &dyn Backend, globals: &Globals, a: &ProbeArgs) -> Result<Value, CoreError> {
    if backend.dry_run_only() {
        return Err(
            unavailable(json!({ "mode": "offline", "operation": "media.probeDeep" })).with_fix(
                "fin media probe decodes on the server: start finisherd and run it again",
            ),
        );
    }
    let branch = target_branch(backend, globals)?;
    backend.project_call(
        "media.probeDeep",
        json!({ "branch": branch, "asset": a.asset }),
    )
}
