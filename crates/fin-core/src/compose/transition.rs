//! `compose::transition_with_links` (plan 4C Decision 22): a transition at a cut, and a
//! crossfade at each cut between the link partners of its two items.

use serde_json::json;

use crate::compose::Composed;
use crate::error::CoreError;
use crate::model::{Curve, Project, TrackKind, TransitionKind};
use crate::ops::util::{find_item, members, touching};
use crate::ops::{Op, TransitionSet};
use crate::time::Rational;

/// Ops for a transition between items `a` and `b` (on `a`'s track) of `kind`, `duration` and
/// `curve` (`$transition`), then, when `linked`, one between each pair of their link partners
/// that share a track and touch there (`$transition2`, …): a `crossfade` on an audio track,
/// a `dissolve` on a video one (caption tracks take none). `echo` is `{ transitions: [{ id, track, between }], skipped: [{ track,
/// between }] }`: `skipped` names the partner pairs on one track that do not touch.
/// `NOT_FOUND` for an unknown item; everything else is the ops' own validation.
pub fn transition_with_links(
    project: &Project,
    a: &str,
    b: &str,
    kind: TransitionKind,
    duration: Rational,
    curve: Curve,
    linked: bool,
) -> Result<Composed, CoreError> {
    let track_of = |id: &str| -> Result<crate::ops::util::TrackAt, CoreError> {
        find_item(project, id)
            .map(|(t, _)| t)
            .ok_or_else(|| crate::ops::util::not_found("item", id))
    };
    let ta = track_of(a)?;
    let mut ops: Vec<Op> = Vec::new();
    let mut made = Vec::new();
    let mut skipped = Vec::new();
    let mut set = |track: &str, between: [String; 2], kind: TransitionKind| {
        let id = match ops.len() {
            0 => "$transition".to_string(),
            n => format!("$transition{}", n + 1),
        };
        made.push(json!({ "id": id, "track": track, "between": between }));
        ops.push(
            TransitionSet {
                id,
                track: track.to_string(),
                between,
                kind,
                duration,
                curve,
            }
            .into(),
        );
    };
    set(&ta.get(project).id, [a.to_string(), b.to_string()], kind);
    if linked {
        let partners_b = members(project, b)?;
        for pa in members(project, a)?.into_iter().skip(1) {
            let tp = track_of(&pa)?;
            let Some(pb) = partners_b
                .iter()
                .skip(1)
                .find(|pb| track_of(pb).is_ok_and(|t| t == tp))
            else {
                continue;
            };
            let track = tp.get(project);
            let between = [pa.clone(), pb.clone()];
            if touching(track, &between)?.is_none() {
                skipped.push(json!({ "track": track.id, "between": between }));
                continue;
            }
            let kind = match track.kind {
                TrackKind::Audio => TransitionKind::Crossfade,
                TrackKind::Video => TransitionKind::Dissolve,
                TrackKind::Caption => continue,
            };
            set(&track.id, between, kind);
        }
    }
    Ok(Composed {
        ops,
        echo: json!({ "transitions": made, "skipped": skipped }),
    })
}
