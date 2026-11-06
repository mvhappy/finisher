//! Looking things up in a project.

use crate::error::CoreError;
use crate::ids::IdKind;
use crate::model::{Asset, Clip, Project};
use crate::ops::util::not_found;
use crate::time::Rational;

pub(crate) fn exists(project: &Project, kind: IdKind, id: &str) -> bool {
    match kind {
        IdKind::Asset => project.assets.contains_key(id),
        IdKind::Clip => project
            .timeline
            .tracks
            .iter()
            .any(|t| t.items.iter().any(|c| c.id == id)),
        IdKind::Track => project.timeline.tracks.iter().any(|t| t.id == id),
        IdKind::Timeline => project.timeline.id == id,
        // Branches are not part of the model.
        IdKind::Branch => false,
    }
}

pub(crate) fn track_index(project: &Project, id: &str) -> Result<usize, CoreError> {
    project
        .timeline
        .tracks
        .iter()
        .position(|t| t.id == id)
        .ok_or_else(|| not_found("track", id))
}

/// `(track index, item index)` of clip `id`.
pub(crate) fn clip_index(project: &Project, id: &str) -> Result<(usize, usize), CoreError> {
    for (ti, track) in project.timeline.tracks.iter().enumerate() {
        if let Some(ci) = track.items.iter().position(|c| c.id == id) {
            return Ok((ti, ci));
        }
    }
    Err(not_found("clip", id))
}

pub(crate) fn asset<'a>(project: &'a Project, id: &str) -> Result<&'a Asset, CoreError> {
    project.assets.get(id).ok_or_else(|| not_found("asset", id))
}

pub(crate) fn end(clip: &Clip) -> Result<Rational, CoreError> {
    clip.start.checked_add(clip.duration)
}
