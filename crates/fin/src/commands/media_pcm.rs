//! `fin media pcm <asset> [--recipe pcm48|pcm16m] [--no-wait]` (plan 6C Task 18): an asset's
//! audio as a PCM feed in the server's media cache (`media.pcm`): `pcm48` (default, 48 kHz in
//! the source's own layout: mono stays one channel) or `pcm16m` (16 kHz mono, the analysis
//! feed). `fin` links no media crate: it sends the request and prints the answer,
//! `{asset, recipe, key, state, cached?, job?, result?}`: a cache hit at once (`state: ready,
//! cached: true`); otherwise the server waits up to 10 minutes for the job (`result` is its
//! `PcmResult`), or with `--no-wait` answers `state: making` and the `job`. A waiting call is a
//! long call, as `fin media proxy`'s.

use std::time::Duration;

use clap::{Args, ValueEnum};
use fin_core::CoreError;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::{Globals, target_branch};
use crate::backend::Backend;
use crate::client::Client;
use crate::endpoint;
use crate::output::unavailable;

/// How long the server waits for the job (finisherd's `PCM_WAIT_MS`).
const PCM_WAIT: Duration = Duration::from_secs(10 * 60);
/// The server's answer after its wait: a margin for it to reach fin.
const WAIT_SLACK: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum RecipeArg {
    Pcm48,
    Pcm16m,
}

/// Make (or find) an asset's PCM feed.
#[derive(Debug, Clone, Args, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PcmArgs {
    /// The asset's id.
    pub asset: String,
    /// "pcm48" (default) or "pcm16m".
    #[arg(long, value_name = "pcm48|pcm16m")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recipe: Option<RecipeArg>,
    /// Answer at once with the job making it.
    #[arg(long)]
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub no_wait: bool,
}

pub fn run(backend: &dyn Backend, globals: &Globals, a: &PcmArgs) -> Result<Value, CoreError> {
    if backend.dry_run_only() {
        return Err(
            unavailable(json!({ "mode": "offline", "operation": "media.pcm" }))
                .with_fix("fin media pcm is made on the server: start finisherd and run it again"),
        );
    }
    let branch = target_branch(backend, globals)?;
    let mut req = json!({ "branch": branch, "asset": a.asset });
    if let Some(r) = a.recipe {
        req["recipe"] = json!(r);
    }
    if a.no_wait {
        req["wait"] = json!(false);
        return backend.project_call("media.pcm", req);
    }
    req["projectId"] = json!(backend.project_id()?);
    // A long call with its own timeout: outside the overall deadline, as `fin job wait`.
    let mut client =
        Client::connect(&endpoint::resolve()?, PCM_WAIT + WAIT_SLACK)?.without_deadline();
    client.call("media.pcm", req)
}
