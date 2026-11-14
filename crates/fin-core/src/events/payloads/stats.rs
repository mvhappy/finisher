//! `rpc.stats` and `eval.stats` (Decision 19): per-minute histograms of high-rate timings, one
//! entry per key per window.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::Counts;
use crate::events::approx::approx_struct;
use crate::events::class::{Code, Name};
use crate::events::names;

/// The number of log₂ buckets: 1 µs to 2⁴⁰ µs.
pub const BUCKETS: usize = 41;

/// One window's timings for one key. `buckets[i]` counts the samples with
/// `2^i ≤ micros < 2^(i+1)` (`buckets[0]` also counts 0, the last bucket everything above);
/// trailing empty buckets are omitted.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Histogram {
    /// The window's start (ms since the epoch) and length.
    pub start: u64,
    pub window_ms: u64,
    pub count: u64,
    /// Error code → count.
    #[serde(default, skip_serializing_if = "Counts::is_empty")]
    pub errors: Counts<Code>,
    pub max_micros: u64,
    pub buckets: Vec<u64>,
}
approx_struct!(Histogram {
    start,
    window_ms,
    count,
    errors,
    max_micros,
    buckets
});

impl Histogram {
    /// The bucket of a sample.
    pub fn bucket(micros: u64) -> usize {
        if micros == 0 {
            0
        } else {
            (63 - micros.leading_zeros() as usize).min(BUCKETS - 1)
        }
    }
}

/// `rpc.stats`: one non-stream RPC method (`RPC_METHODS`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RpcStats {
    #[serde(deserialize_with = "names::rpc_method")]
    pub key: Name,
    pub histogram: Histogram,
}
approx_struct!(RpcStats { key, histogram });

/// `eval.stats`: one of `fin-node`'s eval calls (`EVAL_KEYS`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct EvalStats {
    #[serde(deserialize_with = "names::eval_key")]
    pub key: Name,
    pub histogram: Histogram,
}
approx_struct!(EvalStats { key, histogram });
