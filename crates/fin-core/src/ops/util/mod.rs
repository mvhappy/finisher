//! Shared lookups, checks and ripple helpers for the op modules.

use serde_json::json;

use crate::error::CoreError;
use crate::ops::Op;

mod alone;
mod caption;
mod carve;
mod check;
mod content;
pub(crate) mod contents;
mod group;
mod limits;
mod lock;
mod lookup;
pub(crate) mod param_check;
pub(crate) mod paths;
mod prune;
mod ripple;
pub(crate) mod roll;
pub(crate) mod settings;
mod touched;
mod values;

pub(crate) use alone::{dissolve_alone, split_dissolved};
pub(crate) use caption::{
    base_style, block_mut, caption_at, check_caption, check_caption_words, check_name,
    check_overrides, check_style_value, merge_source, project_style, split_source,
};
pub(crate) use carve::{carve, check_carve};
pub(crate) use check::{
    check_frame, check_kind, check_source, check_speed, check_track, check_tracks, invalid_range,
    kind_name, media_duration,
};
pub(crate) use content::{move_start_edge, shift_content};
pub(crate) use group::{group_items, members};
pub(crate) use limits::{Limits, check_delta, edge_limits, intersect};
pub(crate) use lock::check_unlocked;
pub(crate) use lookup::{
    TrackAt, asset, end, exists, find_item, find_transition, item_index, items, track_index, tracks,
};
pub(crate) use prune::{check_transitions, prune, sort_transitions, touching};
pub(crate) use ripple::{OpDiff, clip_containing, insert_inside, place, ripple};
pub(crate) use touched::touched_tracks;
pub(crate) use values::{
    body as normalize_body, interp as normalize_interp, normalize_item, param as normalize_value,
};

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

/// The frame length of the timeline `at` is on.
pub(crate) fn frame_of(
    project: &crate::model::Project,
    at: TrackAt,
) -> Result<crate::time::Rational, CoreError> {
    crate::time::frame_duration(project.timelines[at.timeline].rate)
}
