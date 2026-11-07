//! The walker's deadlines (Decision 18), injected so tests scale them instead of sleeping
//! through them (plan: "worker timeouts are scaled through test-hooks params, never slept").

use std::time::Duration;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// How long the walker waits.
///
/// - `first_stat`: the root's first stat of a walk, spin-up included (60 s); past it the
///   root is `timeout` and the walk answers offline;
/// - `entry`: one step of a directory read (its open, a batch of names, one entry's stat;
///   30 s); a read with no step for that long leaves the directory `unreadable` with code
///   `TIMEOUT` and the walk goes on (its thread is left behind);
/// - `flush`: a batch holding anything is handed to the sink at least this often (5 s), so a
///   slow walk still reports progress per directory.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Timing {
    #[serde(with = "ms")]
    #[schemars(with = "u64")]
    pub first_stat: Duration,
    #[serde(with = "ms")]
    #[schemars(with = "u64")]
    pub entry: Duration,
    #[serde(default = "default_flush", with = "ms")]
    #[schemars(with = "u64")]
    pub flush: Duration,
}

pub const FIRST_STAT: Duration = Duration::from_secs(60);
pub const ENTRY: Duration = Duration::from_secs(30);
pub const FLUSH: Duration = Duration::from_secs(5);

fn default_flush() -> Duration {
    FLUSH
}

impl Default for Timing {
    fn default() -> Self {
        Self {
            first_stat: FIRST_STAT,
            entry: ENTRY,
            flush: FLUSH,
        }
    }
}

impl Timing {
    /// Every deadline divided by `factor` (tests: `Timing::scaled(1000)` makes 60 s 60 ms).
    pub fn scaled(factor: u32) -> Self {
        let d = Self::default();
        Self {
            first_stat: d.first_stat / factor,
            entry: d.entry / factor,
            flush: d.flush / factor,
        }
    }
}

/// Durations as whole milliseconds on the wire (`timing` in the scan params, test-hooks).
mod ms {
    use std::time::Duration;

    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(d: &Duration, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_u64(u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Duration, D::Error> {
        u64::deserialize(d).map(Duration::from_millis)
    }
}
