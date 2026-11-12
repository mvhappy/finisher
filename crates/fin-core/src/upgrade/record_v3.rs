//! Record step v3 → v4 (sub-project 4 spec §5.13). **Frozen**: pinned by
//! `tests/upgrade_golden.rs`.
//!
//! Schema 4 adds a graphic's `sourceIn` and the audio params `fadeInOffset`/`fadeOutOffset`
//! (and their registry paths), each 0 by default and omitted at 0. A v3 record carries none of
//! them, and what it does carry means the same in v4, so it is a valid v4 record as it is: the
//! step is the identity, and the runner stamps `schemaVersion: 4` (and
//! `intent.schemaVersion`). The bump exists so an older reader refuses a v4 record
//! (`NEWER_FORMAT`) instead of dropping the new fields.

use serde_json::Value;

use crate::error::CoreError;

pub(crate) fn run(v: Value) -> Result<Value, CoreError> {
    Ok(v)
}
