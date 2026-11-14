//! Record step v5 → v6 (sub-project 6 spec §8.4, plan 6C Task 5). **Frozen**: written on
//! `serde_json::Value` and pinned by `tests/upgrade_golden.rs`.
//!
//! Schema 6 adds the audio effect kinds to the registry and no op or field. A v5 record is a
//! valid v6 record as it is, an `effect.add` of a kind named like an audio one included (v5
//! stored it as an unknown kind's params, which v6 reads the same way): the step is the
//! identity, and the runner stamps `schemaVersion: 6` (and `intent.schemaVersion`). The bump
//! exists so an older reader refuses a v6 record (`NEWER_FORMAT`) instead of treating its
//! audio effects as unknown kinds.

use serde_json::Value;

use crate::error::CoreError;

pub(crate) fn run(v: Value) -> Result<Value, CoreError> {
    Ok(v)
}
