//! `fonts.fetch` (Decision 27): one per font fetch. The entry is keyed-hashed: a pinned
//! family may be a user's local font, and the catalog is not in `fin-core`'s tables.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::events::approx::approx_struct;
use crate::events::class::{Code, Hashed};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct FontsFetch {
    pub entry: Hashed,
    pub bundled: bool,
    pub bytes: u64,
    pub micros: u64,
    pub outcome: FetchOutcome,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Code")]
    pub code: Option<Code>,
}
approx_struct!(FontsFetch {
    entry,
    bundled,
    bytes,
    micros,
    outcome,
    code
});

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum FetchOutcome {
    Ok,
    Failed,
    Cached,
}
