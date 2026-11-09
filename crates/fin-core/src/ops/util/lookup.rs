//! Looking things up in a project, across all its timelines.

use crate::error::CoreError;
use crate::ids::IdKind;
use crate::model::{Asset, Item, Project, Track};
use crate::ops::util::not_found;
use crate::time::Rational;

/// Where a track is: its timeline's index and its index in that timeline.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TrackAt {
    pub(crate) timeline: usize,
    pub(crate) track: usize,
}

impl TrackAt {
    pub(crate) fn get(self, project: &Project) -> &Track {
        &project.timelines[self.timeline].tracks[self.track]
    }

    pub(crate) fn get_mut(self, project: &mut Project) -> &mut Track {
        &mut project.timelines[self.timeline].tracks[self.track]
    }
}

/// Every track of every timeline, in order.
pub(crate) fn tracks(project: &Project) -> impl Iterator<Item = &Track> {
    project.timelines.iter().flat_map(|tl| tl.tracks.iter())
}

/// Every item of every track.
pub(crate) fn items(project: &Project) -> impl Iterator<Item = &Item> {
    tracks(project).flat_map(|t| t.items.iter())
}

/// Whether an entity of `kind` with `id` exists. For item kinds any item with `id` counts,
/// whatever its kind, so no two items share an id.
pub(crate) fn exists(project: &Project, kind: IdKind, id: &str) -> bool {
    match kind {
        IdKind::Asset => project.assets.contains_key(id),
        IdKind::Clip
        | IdKind::Caption
        | IdKind::Graphic
        | IdKind::Image
        | IdKind::Adjustment
        | IdKind::Nested => items(project).any(|c| c.id == id),
        IdKind::Track => tracks(project).any(|t| t.id == id),
        IdKind::Timeline => project.timelines.iter().any(|tl| tl.id == id),
        IdKind::Marker => project
            .timelines
            .iter()
            .any(|tl| tl.markers.iter().any(|m| m.id == id)),
        IdKind::Transition => tracks(project).any(|t| t.transitions.iter().any(|x| x.id == id)),
        IdKind::Bin => project.bins.contains_key(id),
        IdKind::Effect => items(project).any(|c| c.common.effects.iter().any(|e| e.id == id)),
        IdKind::Link => items(project).any(|c| c.common.link.as_deref() == Some(id)),
        IdKind::Style => project.styles.contains_key(id),
        IdKind::Keyframe => {
            items(project).any(|c| c.common.keyframes.values().flatten().any(|k| k.id == id))
        }
        // Branches are not part of the model.
        IdKind::Branch => false,
    }
}

pub(crate) fn track_index(project: &Project, id: &str) -> Result<TrackAt, CoreError> {
    for (timeline, tl) in project.timelines.iter().enumerate() {
        if let Some(track) = tl.tracks.iter().position(|t| t.id == id) {
            return Ok(TrackAt { timeline, track });
        }
    }
    Err(not_found("track", id))
}

/// The track and index of item `id`, of any kind (`NOT_FOUND` "item" when no item has it).
pub(crate) fn item_index(project: &Project, id: &str) -> Result<(TrackAt, usize), CoreError> {
    find_item(project, id).ok_or_else(|| not_found("item", id))
}

/// Where item `id` is, if any item has it.
pub(crate) fn find_item(project: &Project, id: &str) -> Option<(TrackAt, usize)> {
    for (timeline, tl) in project.timelines.iter().enumerate() {
        for (track, t) in tl.tracks.iter().enumerate() {
            if let Some(i) = t.items.iter().position(|c| c.id == id) {
                return Some((TrackAt { timeline, track }, i));
            }
        }
    }
    None
}

/// Where the track holding transition `id` is, if any.
pub(crate) fn find_transition(project: &Project, id: &str) -> Option<TrackAt> {
    for (timeline, tl) in project.timelines.iter().enumerate() {
        for (track, t) in tl.tracks.iter().enumerate() {
            if t.transitions.iter().any(|x| x.id == id) {
                return Some(TrackAt { timeline, track });
            }
        }
    }
    None
}

pub(crate) fn asset<'a>(project: &'a Project, id: &str) -> Result<&'a Asset, CoreError> {
    project.assets.get(id).ok_or_else(|| not_found("asset", id))
}

pub(crate) fn end(item: &Item) -> Result<Rational, CoreError> {
    item.end()
}
