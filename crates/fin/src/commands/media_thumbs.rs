//! `fin media thumbs <asset> --at <t>[,<t>…] | --strip` (plan 6B-1 Task 15, Decision 17):
//! PNG thumbnails of a video asset at source times, or its filmstrip, made by the server's
//! media worker (`media.thumbs`) into the media cache. `fin` links no media crate: it parses
//! the times, sends the request and prints the answer.
//!
//! - `--at`: source times as anywhere else (`12.4s`, `00:12.4`, `62/5`; seconds in the asset,
//!   at most 64), `--max-side` the longest side of each (default 256, at most 512).
//! - `--strip`: one sprite PNG per 10 s of the asset, a tile a second, `--tile-height` tall
//!   (default 90) and a `strip.json` index.
//! - Answers `{asset, mode, key, state, cached?, job?, files?}`: a cache hit at once
//!   (`state: ready, cached: true`); otherwise the server waits up to 10 minutes for the job,
//!   or with `--no-wait` answers `state: making` and the `job`. `files` lists the entry's files
//!   (`media.url` turns one into a URL a client can fetch). A waiting call is a long call (as
//!   `fin media proxy`): its own connection, outside the overall deadline.

use std::time::Duration;

use clap::Args;
use fin_core::{CoreError, Rational};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::{Globals, target_branch};
use crate::addr;
use crate::backend::Backend;
use crate::client::Client;
use crate::endpoint;
use crate::output::{unavailable, usage};

/// How long the server waits for the job (finisherd's `PROXY_WAIT_MS`).
const WAIT: Duration = Duration::from_secs(10 * 60);
/// The server's answer after its wait: a margin for it to reach fin.
const WAIT_SLACK: Duration = Duration::from_secs(30);

/// Thumbnails of a video asset at some times, or its filmstrip.
#[derive(Debug, Clone, Args, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ThumbsArgs {
    /// The asset's id.
    pub asset: String,
    /// Source times, comma separated (12.4s, 00:12.4, 62/5): one thumbnail each, at most 64.
    #[arg(long, value_name = "T[,T…]")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<String>,
    /// The filmstrip: a sprite PNG per 10 s, a tile per second, and its index.
    #[arg(long)]
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub strip: bool,
    /// With --at: the longest side of a thumbnail in pixels (default 256, at most 512).
    #[arg(long, value_name = "PX")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_side: Option<u32>,
    /// With --strip: a tile's height in pixels (default 90).
    #[arg(long, value_name = "PX")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tile_height: Option<u32>,
    /// Answer at once with the job making it.
    #[arg(long)]
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub no_wait: bool,
}

fn server_only() -> CoreError {
    unavailable(json!({ "mode": "offline", "operation": "media.thumbs" }))
        .with_fix("fin media thumbs is made on the server: start finisherd and run it again")
}

fn bad(message: &str) -> CoreError {
    usage(
        format!("media thumbs: {message}"),
        json!({ "command": "media.thumbs" }),
        "fin media thumbs <asset> --at <t>[,<t>…] [--max-side <px>] | --strip [--tile-height <px>]",
    )
}

/// The request's mode fields, checked: `--at` xor `--strip`, each option with its own mode.
fn mode_fields(a: &ThumbsArgs) -> Result<Value, CoreError> {
    match (&a.at, a.strip) {
        (Some(_), true) | (None, false) => Err(bad(
            "give --at <times> or --strip, not both and not neither",
        )),
        (Some(at), false) => {
            if a.tile_height.is_some() {
                return Err(bad("--tile-height goes with --strip"));
            }
            let one = Rational::from_int(1);
            let times = at
                .split(',')
                .map(|t| addr::time_at(one, "at", t.trim()))
                .collect::<Result<Vec<_>, _>>()?;
            let mut m = json!({ "at": times });
            if let Some(s) = a.max_side {
                m["maxSide"] = json!(s);
            }
            Ok(m)
        }
        (None, true) => {
            if a.max_side.is_some() {
                return Err(bad("--max-side goes with --at"));
            }
            let mut m = json!({ "strip": true });
            if let Some(h) = a.tile_height {
                m["tileHeight"] = json!(h);
            }
            Ok(m)
        }
    }
}

pub fn run(backend: &dyn Backend, globals: &Globals, a: &ThumbsArgs) -> Result<Value, CoreError> {
    let mut req = mode_fields(a)?;
    if backend.dry_run_only() {
        return Err(server_only());
    }
    let branch = target_branch(backend, globals)?;
    req["branch"] = json!(branch);
    req["asset"] = json!(a.asset);
    if a.no_wait {
        req["wait"] = json!(false);
        return backend.project_call("media.thumbs", req);
    }
    req["projectId"] = json!(backend.project_id()?);
    // A long call with its own timeout: outside the overall deadline, as `fin media proxy`.
    let mut client = Client::connect(&endpoint::resolve()?, WAIT + WAIT_SLACK)?.without_deadline();
    client.call("media.thumbs", req)
}
