//! Track lock (plan 4C Decision 26): an op on a locked track's items is `TRACK_LOCKED`. It is
//! policy: replay applies whatever was committed.

use serde_json::json;

use crate::error::CoreError;
use crate::model::Track;
use crate::ops::Bindings;

/// `TRACK_LOCKED` when `track` is locked and policy runs; `item` names the item the op
/// changes, if any.
pub(crate) fn check_unlocked(
    track: &Track,
    item: Option<&str>,
    bindings: &Bindings,
) -> Result<(), CoreError> {
    if !track.lock || !bindings.policy() {
        return Ok(());
    }
    Err(
        CoreError::new("TRACK_LOCKED", format!("track {} is locked", track.id))
            .with_details(json!({ "track": track.id, "item": item }))
            .with_fix(format!("unlock track {} first", track.id)),
    )
}
