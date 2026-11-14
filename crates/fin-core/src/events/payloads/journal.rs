//! `journal.dropped` (Decision 18): what the journal dropped since its last report.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::events::approx::approx_struct;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct JournalDropped {
    pub count: u64,
    /// The dropped entries' `approx_bytes`.
    pub bytes: u64,
    /// The start of the counting window (ms since the epoch).
    pub since: u64,
}
approx_struct!(JournalDropped {
    count,
    bytes,
    since
});
