//! Shared lookups, checks and ripple helpers for the op modules.

use serde_json::json;

use crate::error::CoreError;
use crate::ops::Op;

mod check;
mod lookup;
mod ripple;

pub(crate) use check::{check_kind, check_source, check_tracks, invalid_range};
pub(crate) use lookup::{asset, clip_index, end, exists, track_index};
pub(crate) use ripple::{OpDiff, clip_containing, insert_inside, place, ripple};

pub(crate) fn not_found(kind: &str, id: &str) -> CoreError {
    CoreError::new("NOT_FOUND", format!("{kind} {id} does not exist"))
        .with_details(json!({ "id": id, "kind": kind }))
        .with_fix(format!("use the id of an existing {kind}"))
}

/// Raised when a module is handed an op of another kind (a programming error, not user input).
pub(crate) fn wrong_op(op: &Op) -> CoreError {
    CoreError::new(
        "INVALID_OP",
        format!("{} dispatched to the wrong module", op.name()),
    )
    .with_details(json!({ "op": op.name() }))
}
