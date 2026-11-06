//! Record step v1 → v2 (plan 4C Decision 5). For now an identity step: model v2 keeps the v1
//! op shapes (`clip.*`, and `asset.add` without `kind`), so a v1 record reads as it is and
//! only its `schemaVersion` (and `intent.schemaVersion`) is stamped 2 by the runner. Plan 4C
//! Task 7 fills it (`clip.*` → `item.*`, `asset.add` gains `kind: "media"`) and freezes it.

use serde_json::Value;

use crate::error::CoreError;

pub(crate) fn run(v: Value) -> Result<Value, CoreError> {
    Ok(v)
}
