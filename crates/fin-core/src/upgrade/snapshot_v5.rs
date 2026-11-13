//! Snapshot step v5 → v6 (sub-project 6 spec §8.4, plan 6C Task 5). **Frozen**: pinned by
//! `tests/upgrade_golden.rs`; it never changes once it ships, whatever later models look like.
//!
//! Schema 6 adds the audio effect kinds `eq`, `compressor`, `gate` and `deEsser` to the
//! registry and nothing to the model's shape: an effect is still `{id, kind, enabled, params}`.
//! So a v5 project is a valid v6 project as it is: the step is the identity, and the runner
//! stamps `schemaVersion: 6`.

use serde_json::Value;

use crate::error::CoreError;

pub(crate) fn run(v: Value) -> Result<Value, CoreError> {
    Ok(v)
}
