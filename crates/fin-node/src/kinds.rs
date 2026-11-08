//! Kind lookups for TypeScript emitters (sub-project 7A Task 11): whether a kind of fin-core's
//! registry has an emitter yet. The outcome tracker emits `review.verdict` only once 5E gives
//! that kind a version (a lookup, not a flag: spec §12.5).

use fin_core::events::{Source, kinds};
use napi_derive::napi;

/// The newest payload version of kind `name` that `source` (`agent`, `finisherd`, …) may emit,
/// or `null` when the registry has no such kind, only a reserved one (version 0), or none that
/// source may send.
#[napi(catch_unwind)]
pub fn kind_version(name: String, source: String) -> Option<u32> {
    let source: Source = serde_json::from_value(serde_json::Value::String(source)).ok()?;
    kinds()
        .iter()
        .filter(|k| k.name == name && !k.reserved && k.sources.contains(&source))
        .map(|k| u32::from(k.version))
        .max()
}
