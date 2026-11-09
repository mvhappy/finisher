//! Cherry-pick and revert (spec §4.4; plan 4B Decisions 18, 24, Task 14).
//!
//! - **Cherry-pick** ([`plan_cherry_pick`]) is the merge machinery for one record: the record
//!   is re-run at the target's head over the steps that lead there from its source frame (the
//!   source path before it inverted, the target path with its mirrors: [`Paths`]), keeping its
//!   entity ids. The copy is a `cherryPick` record whose `origin` is the record's origin
//!   **root** ([`origin::root`]): a pick of a pick, or of a merge copy, names the record first
//!   written, so "already present" is one test everywhere. Picking a record the target already
//!   holds (its root on the target's lineage, or a copy of that root there) plans nothing, so
//!   a second pick of the same record is a no-op.
//! - **Revert** ([`plan_revert`]) is one `revert` record holding the inverses of a merge
//!   group's records, newest first, in the frame right after the group's last record, rebased
//!   over the branch's records since (mirrors included: an undo of a unit inside, say). A
//!   group whose effect is already gone (reverted, or undone, and not brought back by an undo
//!   of that or a redo; a copy of a revert counts as the revert, by origin root) plans
//!   nothing. A revert that conflicts lists every group record whose inverse cannot be carried
//!   to the head, as a merge lists its candidates.

use std::collections::{HashMap, HashSet};

use serde_json::json;

use super::merge::{
    Binders, Paths, common_point, on_skipped, path_after, rerun_mode, unit_record_in,
};
use super::origin::{Roots, holds, root};
use super::undo::{intent, try_rebase_over};
use super::{LogRec, LogView, Outcome, Planned};
use crate::error::CoreError;
use crate::ids::IdAlloc;
use crate::rebase::{Conflict, Conflicts, Rebase, reasons, rebase_apply};
use crate::record::{Link, RecordKind};
use crate::replay::BoundIds;
use crate::tx::Tx;

/// The `cherryPick` record that copies record `origin` into branch `into` (an id), by
/// `author`, as `tx_id` (module docs); `Nothing(ALREADY_PRESENT)` when `into` already holds
/// it. The link's
/// `origin_hash` is left `None`: the store fills in the root record's txHash.
///
/// Errors: `NOT_FOUND` (`details.kind` `tx`) for an unknown record; `INVALID_OP` for a record
/// with no ops to copy (`checkpoint`, `branch`, `branchSet`) or branches that share no
/// history; `CONFLICT` (one conflict) when it cannot be re-run at the head: an undo, redo or
/// revert tied to the target's own records is `UNDOES_TARGET_RECORDS`, a conflict blamed on an
/// earlier source record the target does not hold is `DEPENDS_ON_SKIPPED`, and so is an undo,
/// redo or revert whose unit holds such a record.
pub fn plan_cherry_pick(
    log: &dyn LogView,
    origin: &str,
    into: &str,
    author: &str,
    tx_id: &str,
) -> Result<Outcome, CoreError> {
    let present = Ok(Outcome::Nothing(reasons::ALREADY_PRESENT));
    let Some(rec) = log.find(origin)? else {
        return Err(
            CoreError::new("NOT_FOUND", format!("record {origin} does not exist"))
                .with_details(json!({ "kind": "tx", "id": origin }))
                .with_fix("name a record's txId, as a commit or merge printed it"),
        );
    };
    if !(rec.record.kind.is_content() || rec.record.kind == RecordKind::Merge) {
        return Err(CoreError::new(
            "INVALID_OP",
            format!("record {origin} has no edits to cherry-pick"),
        )
        .with_details(json!({ "txId": origin })));
    }
    let (source_l, target_l) = (log.lineage(&rec.branch)?, log.lineage(into)?);
    let c = common_point(&source_l, &target_l);
    if c.0.is_empty() {
        return Err(CoreError::new(
            "INVALID_OP",
            format!("branches {} and {into} share no history", rec.branch),
        )
        .with_details(json!({ "from": rec.branch, "into": into })));
    }
    let (root_id, root_rec) = root(log, &rec)?;
    if root_rec
        .as_ref()
        .is_some_and(|r| holds(&target_l, &r.branch, r.seq))
    {
        return present;
    }
    let source = path_after(log, &source_l, &c)?;
    let target = path_after(log, &target_l, &c)?;
    let Some(i) = source.iter().position(|r| r.record.tx_id == origin) else {
        // At or below the common point: the target's lineage holds it.
        return present;
    };
    let roots = Roots::new(log, &target_l, &source, &target)?;
    if roots.present[i] {
        return present;
    }
    if roots.undoes_target[i] {
        let c = Conflict::new(origin, 0, reasons::UNDOES_TARGET_RECORDS, None);
        return Err(Conflicts::one(c).into());
    }
    // An undo, redo or revert whose unit the target does not hold would apply an inverse of
    // what the target never had (plan 4B Task 20).
    if let Some(dep) = unit_record_in(&source[..i], &rec.record, |t| {
        source[..i]
            .iter()
            .position(|s| s.record.tx_id == t)
            .is_some_and(|j| !roots.present[j])
    }) {
        let c = Conflict::new(origin, 0, reasons::DEPENDS_ON_SKIPPED, Some(dep));
        return Err(Conflicts::one(c).into());
    }
    let steps = Paths::new(&source, &target, &roots)?.steps(i, &[]);
    let same_origin = Binders::new(&source, &roots.source, &target, &roots.target)
        .same_origin(&rec.committed()?)?;
    let r = &rec.record;
    let tx = Tx {
        tx_id: tx_id.to_string(),
        ..r.intent.clone()
    };
    let mode = rerun_mode(r);
    let absorbs = super::absorbs::of_ops(log, &rec)?;
    let reads = super::source_frame(log, &rec, &tx, mode)?;
    let rebase = Rebase {
        tx: &tx,
        own: &rec.diff,
        steps: &steps,
        same_origin: &same_origin,
        env: super::env_of(log),
        absorbs: &absorbs,
        places: &reads.places,
        anchors: &reads.anchors,
    };
    // Earlier source records stand in the steps as unmirrored inverses unless the target
    // holds a copy: what depends on them depends on a record not picked.
    let before: HashSet<&str> = source[..i]
        .iter()
        .map(|s| s.record.tx_id.as_str())
        .collect();
    let (head_seq, head) = log.head(into)?;
    let rebased = BoundIds::new(&r.intent.ops, &r.bindings)
        .map_err(|e| Conflict::from_rerun(&e, tx_id, 0, None))
        .and_then(|mut ids| rebase_apply(&head, &rebase, &mut ids, mode))
        .map_err(|c| CoreError::from(Conflicts::one(on_skipped(c, &before))))?;
    let from = root_rec.map_or_else(|| rec.branch.clone(), |r| r.branch);
    Ok(Outcome::Append(Planned {
        link: Link::CherryPick {
            origin: root_id,
            from,
            origin_hash: None,
            recovers: None,
        },
        intent: Tx {
            tx_id: tx_id.to_string(),
            author: author.to_string(),
            branch: into.to_string(),
            base_rev: head_seq,
            ..rebased.tx
        },
        applied: rebased.applied,
    }))
}

/// The `revert` record of merge group `merge_id` on `branch` (an id), by `author`, as `tx_id`
/// (module docs); `Nothing(ALREADY_REVERTED)` when the group's effect is already gone there.
///
/// Errors: `NOT_FOUND` (`details.kind` `merge`) for an unknown group or one not on `branch`'s
/// lineage; `CONFLICT` when an inverse cannot be carried to the head: `details.conflicts`
/// lists each group record whose inverse fails, newest first, as far as the others can be
/// planned without it (op indexes into the whole revert).
pub fn plan_revert(
    log: &dyn LogView,
    merge_id: &str,
    branch: &str,
    author: &str,
    tx_id: &str,
    ids: &mut dyn IdAlloc,
) -> Result<Outcome, CoreError> {
    let group = log.merge_group(merge_id)?;
    let l = log.lineage(branch)?;
    let last = match group.last() {
        Some(last) if group.iter().all(|r| holds(&l, &r.branch, r.seq)) => last,
        _ => {
            return Err(CoreError::new(
                "NOT_FOUND",
                format!("merge {merge_id} is not on branch {branch}"),
            )
            .with_details(json!({ "kind": "merge", "id": merge_id, "branch": branch }))
            .with_fix("name a merge on this branch's history (--branch)"));
        }
    };
    let since = path_after(log, &l, &(last.branch.clone(), last.seq))?;
    if !in_effect(log, merge_id, &since)? {
        return Ok(Outcome::Nothing(reasons::ALREADY_REVERTED));
    }
    // The group's inverses, newest record first: (first op index, record's inverse).
    let mut parts = Vec::new();
    let mut at = 0;
    for r in group.iter().rev() {
        parts.push((at, r.record.inverse.clone()));
        at += r.record.inverse.len();
    }
    let link = Link::Revert {
        reverts: merge_id.to_string(),
    };
    let head = log.head(branch)?;
    let plan = |keep: &[bool], ids: &mut dyn IdAlloc| {
        let ops = parts
            .iter()
            .zip(keep)
            .filter(|(_, k)| **k)
            .flat_map(|((_, ops), _)| ops.iter().cloned())
            .collect();
        let tx = intent(tx_id, author, branch, last.seq, ops);
        let kept: Vec<&LogRec> = group
            .iter()
            .rev()
            .zip(keep)
            .filter(|(_, k)| **k)
            .map(|(r, _)| r)
            .collect();
        let absorbs = super::absorbs::of_inverses(log, &kept)?;
        try_rebase_over(
            log,
            (head.0, &head.1),
            &since,
            (tx, &absorbs),
            link.clone(),
            ids,
        )
    };
    let mut keep = vec![true; parts.len()];
    let first = match plan(&keep, ids)? {
        Ok(p) => return Ok(Outcome::Append(p)),
        Err(c) => c,
    };
    // List every record that fails: leave each failing one out and plan the rest again (on a
    // scratch counter: nothing of it is kept).
    let mut conflicts = Vec::new();
    let mut c = first;
    loop {
        // `c.op_index` counts the kept ops only: find its record.
        let mut seen = 0;
        let Some(k) = (0..parts.len()).filter(|&k| keep[k]).find(|&k| {
            seen += parts[k].1.len();
            c.op_index < seen
        }) else {
            break;
        };
        let before: usize = (0..k).filter(|&j| keep[j]).map(|j| parts[j].1.len()).sum();
        conflicts.push(Conflict {
            op_index: parts[k].0 + (c.op_index - before),
            ..c
        });
        keep[k] = false;
        if !keep.contains(&true) {
            break;
        }
        match plan(&keep, &mut crate::ids::MemIds::after(&head.1)) {
            Ok(Err(next)) => c = next,
            // The rest plans (or no longer applies in its frame without the failing record).
            Ok(Ok(_)) | Err(_) => break,
        }
    }
    Err(Conflicts::new(conflicts)
        .expect("at least the first conflict")
        .into())
}

/// Whether unit `id` (a txId or mergeId applied before `since`) still has its effect after
/// `since` (oldest first): no later record that inverts it (an undo naming it, a revert of the
/// group, a redo of an undo) is itself in effect. A copy (merge, cherry-pick) inverts what its
/// origin root inverts, and a merge copy is also undone through its group's id.
fn in_effect(log: &dyn LogView, id: &str, since: &[LogRec]) -> Result<bool, CoreError> {
    // Each record of `since`: the unit ids it inverts, and the ids it is undone by.
    let mut inverts: Vec<Vec<String>> = Vec::with_capacity(since.len());
    for r in since {
        let (_, held) = root(log, r)?;
        let base = held.as_ref().unwrap_or(r);
        let b = &base.record;
        inverts.push(match b.kind {
            RecordKind::Undo => b.undoes.clone().unwrap_or_default(),
            RecordKind::Redo => b.redoes.iter().cloned().collect(),
            RecordKind::Revert => b.reverts.iter().cloned().collect(),
            _ => Vec::new(),
        });
    }
    fn names(r: &LogRec) -> Vec<&str> {
        let mut out = vec![r.record.tx_id.as_str()];
        out.extend(r.record.merge_id.as_deref());
        out
    }
    fn on(
        ids: &[&str],
        from: usize,
        since: &[LogRec],
        inverts: &[Vec<String>],
        memo: &mut HashMap<usize, bool>,
    ) -> bool {
        for k in from..since.len() {
            if inverts[k].iter().any(|u| ids.contains(&u.as_str())) {
                let live = match memo.get(&k) {
                    Some(&v) => v,
                    None => {
                        let v = on(&names(&since[k]), k + 1, since, inverts, memo);
                        memo.insert(k, v);
                        v
                    }
                };
                if live {
                    return false;
                }
            }
        }
        true
    }
    Ok(on(&[id], 0, since, &inverts, &mut HashMap::new()))
}
