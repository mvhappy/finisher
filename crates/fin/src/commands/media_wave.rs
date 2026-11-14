//! `fin media wave <asset> [--no-wait]` (plan 6C Task 19): an asset's waveform peaks in the
//! server's media cache (`media.waveform`): `peaks.bin`, per channel `(min, max)` `i16` pairs at
//! 256, 1,024, 4,096, 16,384 and 65,536 samples per peak. `fin` links no media crate: it sends
//! the request and prints the answer, `{asset, key, levels, state, cached?, job?, result?}`:
//! `levels` are the samples per peak of each level and `key` the cache entry; a cache hit answers
//! at once (`state: ready, cached: true`); otherwise the server waits up to 10 minutes for the
//! job (`result` is its `WaveResult`), or with `--no-wait` answers `state: making` and the `job`.
//! A waiting call is a long call, as `fin media pcm`'s.

use std::time::Duration;

use clap::Args;
use fin_core::CoreError;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::{Globals, target_branch};
use crate::backend::Backend;
use crate::client::Client;
use crate::endpoint;
use crate::output::unavailable;

/// How long the server waits for the job (finisherd's `WAVE_WAIT_MS`).
const WAVE_WAIT: Duration = Duration::from_secs(10 * 60);
/// The server's answer after its wait: a margin for it to reach fin.
const WAIT_SLACK: Duration = Duration::from_secs(30);

/// Make (or find) an asset's waveform peaks.
#[derive(Debug, Clone, Args, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WaveArgs {
    /// The asset's id.
    pub asset: String,
    /// Answer at once with the job making it.
    #[arg(long)]
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub no_wait: bool,
}

pub fn run(backend: &dyn Backend, globals: &Globals, a: &WaveArgs) -> Result<Value, CoreError> {
    if backend.dry_run_only() {
        return Err(
            unavailable(json!({ "mode": "offline", "operation": "media.waveform" }))
                .with_fix("fin media wave is made on the server: start finisherd and run it again"),
        );
    }
    let branch = target_branch(backend, globals)?;
    let mut req = json!({ "branch": branch, "asset": a.asset });
    if a.no_wait {
        req["wait"] = json!(false);
        return backend.project_call("media.waveform", req);
    }
    req["projectId"] = json!(backend.project_id()?);
    // A long call with its own timeout: outside the overall deadline, as `fin job wait`.
    let mut client =
        Client::connect(&endpoint::resolve()?, WAVE_WAIT + WAIT_SLACK)?.without_deadline();
    client.call("media.waveform", req)
}
