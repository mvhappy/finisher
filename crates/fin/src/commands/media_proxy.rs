//! `fin media proxy <asset>` and `fin media status <asset>` (plan 6B-1 Task 14): a proxy of a
//! video asset made by the server's media worker (`media.proxy`), and the state of each tier
//! (`media.proxy.status`). `fin` links no media crate: it sends the request and prints the
//! answer.
//!
//! - `proxy`: `--tier 540|1080` (default 540), `--codec h264|av1` (default: H.264 where a hardware
//!   encoder makes it, else AV1; an H.264 proxy on a machine without one is `CODEC_UNAVAILABLE`,
//!   exit 5). Answers `{asset, tier, codec, key, state, cached?, job?, result?}`: a cache hit at
//!   once (`state: ready, cached: true`); otherwise the server waits up to 10 minutes for the job
//!   (`result` is its `ProxyResult`), or with `--no-wait` answers `state: making` and the `job`.
//! - `status`: `{asset, fingerprint, tiers: [{tier, state, key?, codec?, job?, code?}]}`, `state`
//!   one of `ready`, `making`, `failed`, `none`.

use clap::{Args, ValueEnum};
use fin_core::CoreError;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::{Globals, target_branch};
use crate::backend::Backend;
use crate::output::unavailable;

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum, Serialize, Deserialize, JsonSchema)]
pub enum TierArg {
    #[value(name = "540")]
    #[serde(rename = "540")]
    T540,
    #[value(name = "1080")]
    #[serde(rename = "1080")]
    T1080,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum CodecArg {
    H264,
    Av1,
}

/// Make (or find) a proxy of a video asset.
#[derive(Debug, Clone, Args, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProxyArgs {
    /// The asset's id.
    pub asset: String,
    /// "540" (default) or "1080".
    #[arg(long, value_name = "540|1080")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tier: Option<TierArg>,
    /// "h264" or "av1" (default: H.264 where a hardware encoder makes it, else AV1).
    #[arg(long, value_name = "h264|av1")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub codec: Option<CodecArg>,
    /// Answer at once with the job making it.
    #[arg(long)]
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub no_wait: bool,
}

/// The state of an asset's proxies.
#[derive(Debug, Clone, Args, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct StatusArgs {
    /// The asset's id.
    pub asset: String,
}

fn server_only(operation: &str, fix: &str) -> CoreError {
    unavailable(json!({ "mode": "offline", "operation": operation })).with_fix(fix)
}

pub fn proxy(backend: &dyn Backend, globals: &Globals, a: &ProxyArgs) -> Result<Value, CoreError> {
    if backend.dry_run_only() {
        return Err(server_only(
            "media.proxy",
            "fin media proxy is made on the server: start finisherd and run it again",
        ));
    }
    let branch = target_branch(backend, globals)?;
    let mut req = json!({ "branch": branch, "asset": a.asset });
    if let Some(t) = a.tier {
        req["tier"] = json!(t);
    }
    if let Some(c) = a.codec {
        req["codec"] = json!(c);
    }
    if a.no_wait {
        req["wait"] = json!(false);
    }
    backend.project_call("media.proxy", req)
}

pub fn status(
    backend: &dyn Backend,
    globals: &Globals,
    a: &StatusArgs,
) -> Result<Value, CoreError> {
    if backend.dry_run_only() {
        return Err(server_only(
            "media.proxy.status",
            "fin media status asks the server: start finisherd and run it again",
        ));
    }
    let branch = target_branch(backend, globals)?;
    backend.project_call(
        "media.proxy.status",
        json!({ "branch": branch, "asset": a.asset }),
    )
}
