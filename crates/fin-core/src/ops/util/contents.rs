//! What a track holds, as the track and timeline removals and restores see it: recording it
//! in a diff, listing its ids (plan 4C Decision 24), and checking it can come back.

use std::collections::BTreeSet;

use serde_json::json;

use crate::error::CoreError;
use crate::ids::IdKind;
use crate::model::{Project, Track};
use crate::ops::Bindings;
use crate::ops::item_insert::{check_body, resolve_body};
use crate::ops::util::{OpDiff, check_track, check_transitions, normalize_item};
use crate::time::Rational;
use crate::tx::{ChangeKind, EntityKind};

/// Records `track` and everything on it as `change` (added or removed): its items and
/// transitions (each on it), then the track itself when removed, or first when added.
pub(crate) fn record_track(track: &Track, change: ChangeKind, d: &mut OpDiff<'_>) {
    if change == ChangeKind::Added {
        d.record(EntityKind::Track, &track.id, change);
    }
    for item in &track.items {
        d.record_item(&track.id, &item.id, change);
    }
    for t in &track.transitions {
        d.record_on(&track.id, EntityKind::Transition, &t.id, change);
    }
    if change == ChangeKind::Removed {
        d.record(EntityKind::Track, &track.id, change);
    }
}

/// Every id `track` carries, mutably: its own, each item's ([`crate::model::Item::refs_mut`]),
/// and each transition's with the two items it sits between.
pub(crate) fn track_refs_mut(track: &mut Track) -> Vec<&mut String> {
    let mut out = vec![&mut track.id];
    for item in &mut track.items {
        out.extend(item.refs_mut());
    }
    for t in &mut track.transitions {
        out.push(&mut t.id);
        let [a, b] = &mut t.between;
        out.push(a);
        out.push(b);
    }
    out
}

/// The ids [`track_refs_mut`] lists.
pub(crate) fn track_ids(track: &Track) -> Vec<&str> {
    let mut out = vec![track.id.as_str()];
    for item in &track.items {
        out.extend(item.ids());
    }
    for t in &track.transitions {
        out.push(&t.id);
        out.extend(t.between.iter().map(String::as_str));
    }
    out
}

/// A literal id for an entity a restore brings back (`INVALID_OP` for a placeholder: a restore
/// allocates nothing), new in `project` and not already brought back by this op (`seen`).
pub(crate) fn check_returning(
    project: &Project,
    id: &str,
    kind: IdKind,
    bindings: &Bindings,
    seen: &mut BTreeSet<String>,
) -> Result<(), CoreError> {
    if id.starts_with('$') {
        return Err(
            CoreError::new("INVALID_OP", format!("restored id {id} is a placeholder"))
                .with_details(json!({ "id": id }))
                .with_fix("restores name the literal ids of what they bring back"),
        );
    }
    bindings.check_new(project, id, kind)?;
    if !seen.insert(id.to_string()) {
        return Err(
            CoreError::new("DUPLICATE_ID", format!("id {id} is restored twice"))
                .with_details(json!({ "id": id })),
        );
    }
    Ok(())
}

/// `track` as a restore brings it back: its id, its items' and its transitions' ids new (and
/// literal), its item values canonical and their bodies valid on it (assets exist, kinds fit;
/// `frame` is its timeline's), its items not overlapping and its transitions at cuts. Returns
/// it with the references its items name resolved.
pub(crate) fn returning_track(
    project: &Project,
    track: &Track,
    frame: Rational,
    bindings: &Bindings,
    seen: &mut BTreeSet<String>,
) -> Result<Track, CoreError> {
    let mut track = track.clone();
    check_returning(project, &track.id, IdKind::Track, bindings, seen)?;
    if !track.gain.is_finite() {
        return Err(CoreError::new(
            "INVALID_RANGE",
            format!("track {} gain is not a finite number", track.id),
        )
        .with_details(json!({ "path": "gain" }))
        .with_fix("use a finite number"));
    }
    if track.gain == 0.0 {
        track.gain = 0.0;
    }
    for item in &mut track.items {
        check_returning(
            project,
            &item.id,
            IdKind::of_item(&item.body),
            bindings,
            seen,
        )?;
        normalize_item(item)?;
        super::keys::check_key_lists(&item.common.keyframes, [], !bindings.policy())?;
        resolve_body(&mut item.body, bindings)?;
    }
    for item in &track.items {
        check_body(project, &track, &item.body, item.duration, frame, bindings)?;
        let kinds = item.common.effects.iter().map(|e| e.kind.as_str());
        super::check_effect_domains(&track, &item.id, kinds, bindings)?;
    }
    for t in &track.transitions {
        check_returning(project, &t.id, IdKind::Transition, bindings, seen)?;
    }
    check_track(&track)?;
    check_transitions(&track)?;
    Ok(track)
}

/// Declares `track`, its items (with what is nested in them) and its transitions put back
/// whole (`Footprint::puts_back`): a `track.restore`, or a track of a `timeline.restore`.
pub(crate) fn put_back_track(f: &mut crate::rebase::Footprint, track: &Track) {
    f.puts_back(&track.id);
    for i in &track.items {
        f.puts_back(&i.id);
    }
    for t in &track.transitions {
        f.puts_back(&t.id);
    }
}
