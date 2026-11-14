//! `storage.state` (Decision 27): a project became read-only or writable.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::events::approx::approx_struct;
use crate::events::class::Code;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct StorageState {
    pub state: StorageMode,
    // Why (`LEASE_LOST`, `CONFLICT_COPY`, `FORMAT_NEWER`, ...). Not a doc comment: it would
    // inline `Code` in the generated TypeScript.
    pub reason: Code,
}
approx_struct!(StorageState { state, reason });

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum StorageMode {
    ReadOnly,
    Writable,
}
