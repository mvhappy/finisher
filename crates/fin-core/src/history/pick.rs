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
//!   of that or a redo) plans nothing.

use std::collections::{BTreeSet, HashMap, HashSet};

use serde_json::json;

use super::merge::{Paths, TO_HEAD, common_point, on_skipped, path_after};
use super::origin::{root, root_of, target_roots};
use super::undo::{intent, rebase_over};
use super::{Lineage, LogRec, LogView, Planned};
use crate::apply::ApplyMode;
use crate::error::CoreError;
use crate::ids::IdAlloc;
use crate::rebase::{Conflict, Conflicts, Rebase, rebase_apply};
use crate::record::{Link, RecordKind};
use crate::replay::BoundIds;
use crate::tx::Tx;

/// The `cherryPick` record that copies record `origin` into branch `into` (an id), by
/// `author`, as `tx_id` (module docs); `None` when `into` already holds it. The link's
/// `origin_hash` is left `None`: the store fills in the root record's txHash.
///
/// Errors: `NOT_FOUND` (`details.kind` `tx`) for an unknown record; `INVALID_OP` for a record
/// with no ops to copy (`checkpoint`, `branch`, `branchSet`) or branches that share no
/// history; `CONFLICT` (one conflict) when it cannot be re-run at the head: an undo, redo or
/// revert tied to the target's own records is `UNDOES_TARGET_RECORDS`, a conflict blamed on an
/// earlier source record the target does not hold is `DEPENDS_ON_SKIPPED`.
pub fn plan_cherry_pick(
    log: &dyn LogView,
    origin: &str,
    into: &str,
    author: &str,
    tx_id: &str,
) -> Result<Option<Planned>, CoreError> {
    let Some(rec) = log.find(origin)? else {
        return Err(
            CoreError::new("NOT_FOUND", format!("record {origin} does not exist"))
                .with_details(json!({ "kind": "tx", "id": origin }))
                .with_fix("name a record's txId (fin log, fin branch ls)"),
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
        return Ok(None);
    }
    let source = path_after(log, &source_l, &c)?;
    let target = path_after(log, &target_l, &c)?;
    let Some(i) = source.iter().position(|r| r.record.tx_id == origin) else {
        // At or below the common point: the target's lineage holds it.
        return Ok(None);
    };
    for t in &target {
        if root_of(log, t)? == root_id {
            return Ok(None);
        }
    }
    if let Some(&(_, reason)) = super::candidates(&source, &target)
        .excluded
        .iter()
        .find(|(j, _)| *j == i)
    {
        let c = Conflict::new(origin, 0, reason, None);
        return Err(Conflicts::one(c).into());
    }
    let steps = Paths::new(&source, &target)?.steps(i, &[]);
    let same_origin: BTreeSet<String> = target_roots(log, &target)?
        .into_iter()
        .filter(|(t, _)| *t == root_id)
        .flat_map(|(_, ids)| ids)
        .collect();
    let r = &rec.record;
    let tx = Tx {
        tx_id: tx_id.to_string(),
        ..r.intent.clone()
    };
    let mode = match r.kind {
        RecordKind::Edit | RecordKind::CherryPick => ApplyMode::Client,
        _ => ApplyMode::Internal,
    };
    let rebase = Rebase {
        tx: &tx,
        own: &rec.diff,
        steps: &steps,
        same_origin: &same_origin,
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
    Ok(Some(Planned {
        link: Link::CherryPick {
            origin: root_id,
            from,
            origin_hash: None,
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
/// (module docs); `None` when the group's effect is already gone there.
///
/// Errors: `NOT_FOUND` (`details.kind` `merge`) for an unknown group or one not on `branch`'s
/// lineage; `CONFLICT` (one conflict) when an inverse cannot be carried to the head.
pub fn plan_revert(
    log: &dyn LogView,
    merge_id: &str,
    branch: &str,
    author: &str,
    tx_id: &str,
    ids: &mut dyn IdAlloc,
) -> Result<Option<Planned>, CoreError> {
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
    if !in_effect(merge_id, &since) {
        return Ok(None);
    }
    let ops = group
        .iter()
        .rev()
        .flat_map(|r| r.record.inverse.iter().cloned())
        .collect();
    let link = Link::Revert {
        reverts: merge_id.to_string(),
    };
    let tx = intent(tx_id, author, branch, last.seq, ops);
    let (head_seq, head) = log.head(branch)?;
    rebase_over(log, (head_seq, &head), &since, tx, link, ids).map(Some)
}

/// Whether lineage `l` holds the row `seq` of `branch`.
fn holds(l: &Lineage, branch: &str, seq: u64) -> bool {
    l.segs
        .iter()
        .any(|(b, after, through)| b == branch && seq > *after && seq <= through.unwrap_or(TO_HEAD))
}

/// Whether unit `id` (a txId or mergeId applied before `since`) still has its effect after
/// `since` (oldest first): no later record that inverts it (an undo naming it, a revert of the
/// group, a redo of an undo) is itself in effect.
fn in_effect(id: &str, since: &[LogRec]) -> bool {
    fn inverts(r: &LogRec, id: &str) -> bool {
        let r = &r.record;
        match r.kind {
            RecordKind::Undo => r.undoes.iter().flatten().any(|u| u == id),
            RecordKind::Redo => r.redoes.as_deref() == Some(id),
            RecordKind::Revert => r.reverts.as_deref() == Some(id),
            _ => false,
        }
    }
    fn on(id: &str, since: &[LogRec], memo: &mut HashMap<String, bool>) -> bool {
        if let Some(&v) = memo.get(id) {
            return v;
        }
        let mut v = true;
        for (k, r) in since.iter().enumerate() {
            if inverts(r, id) && on(&r.record.tx_id, &since[k + 1..], memo) {
                v = false;
                break;
            }
        }
        memo.insert(id.to_string(), v);
        v
    }
    on(id, since, &mut HashMap::new())
}
