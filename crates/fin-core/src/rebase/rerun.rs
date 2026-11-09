//! A re-run failure of a rebased op as a conflict: its reason, and the one intervening record
//! to blame when there is one (moved out of `engine`).

use std::collections::BTreeSet;

use serde_json::Value;

use crate::error::CoreError;
use crate::model::Project;

use super::conflict::{Conflict, reasons};
use super::engine::PreparedRebase;
use super::footprint::{Footprint, Touch};

/// The op that failed to re-run, as [`rerun_conflict`] reads it: its footprint, the
/// footprints of our earlier ops, and the placeholders bound so far.
pub(super) struct Ours<'a> {
    pub(super) fp: &'a Footprint,
    pub(super) earlier: &'a [Footprint],
    pub(super) bound: &'a std::collections::BTreeMap<String, String>,
}

/// A re-run failure of op `k`. `DUPLICATE_ID` is a copy meeting an id that is already taken
/// (Decision 18). The step to blame is the one unmirrored step (if exactly one) that touched,
/// created or removed an entity ours touches, would create (another pin of the key, for a
/// `FONT_PINNED`) or the error names, or one an earlier op of ours touched to create such an
/// entity (the item our split cut, for an overwrite meeting the right part); tracks and the
/// assets ours only uses are left out, since nearly every step shares them.
pub(super) fn rerun_conflict(
    e: &CoreError,
    r: &PreparedRebase<'_>,
    k: usize,
    ours: &Ours<'_>,
    theirs: &[(&str, &Footprint)],
    head: &Project,
) -> Conflict {
    let mut named: BTreeSet<&str> = ours.fp.touches.iter().map(|(x, _)| x.as_str()).collect();
    // What the op would create: a pin whose key another record pinned since (`FONT_PINNED`) is
    // blamed on that record. A placeholder names no entity here.
    named.extend(
        ours.fp
            .creates
            .iter()
            .map(String::as_str)
            .filter(|x| !x.starts_with('$')),
    );
    for key in ["clip", "item", "id", "asset"] {
        named.extend(e.details[key].as_str());
    }
    if let Some(clips) = e.details["clips"].as_array() {
        named.extend(clips.iter().filter_map(Value::as_str));
    }
    // An entity we created (by placeholder, or the id it bound) is named through what the op
    // that created it touched.
    let creators: Vec<&str> = named
        .iter()
        .flat_map(|x| {
            let placeholder = ours
                .bound
                .iter()
                .find(|(_, id)| id.as_str() == *x)
                .map(|(p, _)| p.as_str());
            [Some(*x), placeholder]
        })
        .flatten()
        .collect();
    for f in ours.earlier {
        if f.creates.iter().any(|c| creators.contains(&c.as_str())) {
            named.extend(f.touches.iter().map(|(x, _)| x.as_str()));
        }
    }
    // A key already at the time ours goes is blamed on the last record that set it there.
    if e.code == "KEYFRAME_EXISTS" {
        return Conflict::from_rerun(e, &r.tx.tx_id, k, keyframe_blame(e, theirs));
    }
    for t in crate::ops::util::tracks(head) {
        named.remove(t.id.as_str());
    }
    let blamed: BTreeSet<&str> = theirs
        .iter()
        .filter(|(_, f)| {
            let changed = f
                .touches
                .iter()
                .map(|(x, _)| x)
                .chain(&f.creates)
                .chain(&f.removes);
            changed.into_iter().any(|x| named.contains(x.as_str()))
        })
        .map(|(tx, _)| *tx)
        .collect();
    let against = (blamed.len() == 1)
        .then(|| blamed.first().map(|t| t.to_string()))
        .flatten();
    if e.code == "DUPLICATE_ID" {
        let id = e.details["id"].as_str().unwrap_or_default();
        let reason = if r.same_origin.contains(id) {
            reasons::ALREADY_PRESENT
        } else {
            reasons::ID_COLLISION
        };
        return Conflict::new(r.tx.tx_id.as_str(), k, reason, against);
    }
    Conflict::from_rerun(e, &r.tx.tx_id, k, against)
}

/// The last record of `theirs` that touched the key a `KEYFRAME_EXISTS` names as `existing`
/// (it inserted, moved or restored it there).
fn keyframe_blame(e: &CoreError, theirs: &[(&str, &Footprint)]) -> Option<String> {
    let d = &e.details;
    let (item, path, existing) = (
        d["item"].as_str()?,
        d["path"].as_str()?,
        d["existing"].as_str()?,
    );
    let entity = crate::ops::util::keys::key_entity(item, path);
    let touch = Touch::Param(crate::ops::util::keys::key_param(path, existing));
    theirs
        .iter()
        .rev()
        .find(|(_, f)| f.touches.iter().any(|(x, t)| *x == entity && *t == touch))
        .map(|(tx, _)| tx.to_string())
}
