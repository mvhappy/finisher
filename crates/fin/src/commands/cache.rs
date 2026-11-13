//! `fin cache status` and `fin cache gc [--to <GiB>]` (6B-1 plan Task 13, Decision 12): the
//! server's media cache. `status` answers `media.cache.status`: `{bytes, entries, capBytes,
//! pinned, byRecipe: {<recipe>: {bytes, entries}}}`. `gc` asks `media.cache.gc` to evict
//! unpinned entries, least recently used first, down to the cap (`media.cacheMaxGiB` in the
//! server's `settings.json`, default `min(50 GiB, 15 % of the volume)`) or to `--to` when lower,
//! and answers `{evicted, freedBytes, bytes}`. Evicting is the owner's (`settings.admin`,
//! human-only): the server refuses `fin` with `HUMAN_ONLY` (exit 7); the owner runs
//! `finisherd cache gc`.

use clap::{Args, Subcommand};
use fin_core::CoreError;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::backend::Backend;
use crate::output::{unavailable, usage};

#[derive(Debug, Clone, Subcommand)]
pub enum CacheCmd {
    /// The media cache's size against its cap, by recipe.
    Status,
    /// Evict unpinned media cache entries down to the cap, or lower with --to.
    Gc(GcArgs),
}

/// Evict down to the cap, or to `to` GiB when that is lower.
#[derive(Debug, Clone, Args, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GcArgs {
    /// The size to evict down to, in GiB (0 or more; the cap when higher).
    #[arg(long, value_name = "GiB")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to: Option<f64>,
}

pub fn run(backend: &dyn Backend, cmd: &CacheCmd) -> Result<Value, CoreError> {
    let (method, payload) = match cmd {
        CacheCmd::Status => ("media.cache.status", json!({})),
        CacheCmd::Gc(a) => match a.to {
            Some(to) if !(to.is_finite() && to >= 0.0) => {
                return Err(usage(
                    "--to needs a number of GiB, 0 or more",
                    json!({ "to": to }),
                    "pass --to <GiB>, e.g. --to 10",
                ));
            }
            Some(to) => ("media.cache.gc", json!({ "toGiB": to })),
            None => ("media.cache.gc", json!({})),
        },
    };
    if backend.dry_run_only() {
        return Err(
            unavailable(json!({ "mode": "offline", "operation": method }))
                .with_fix("fin cache asks the server: start finisherd and run it again"),
        );
    }
    backend.server_call(method, payload)
}
