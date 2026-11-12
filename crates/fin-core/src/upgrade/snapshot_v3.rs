//! Snapshot step v3 → v4 (sub-project 4 spec §5.13). **Frozen**: pinned by
//! `tests/upgrade_golden.rs`; it never changes once it ships, whatever later models look like.
//!
//! Schema 4 adds a graphic's `sourceIn` and an item's `audio.fadeInOffset`/`fadeOutOffset`,
//! each 0 by default and omitted at 0. A v3 project holds none, and 0 is what v3 meant (a
//! graphic's template time was its local time; fades ran from the item's own edges), so it is
//! a valid v4 project as it is: the step is the identity, and the runner stamps
//! `schemaVersion: 4`.

use serde_json::Value;

use crate::error::CoreError;

pub(crate) fn run(v: Value) -> Result<Value, CoreError> {
    Ok(v)
}
