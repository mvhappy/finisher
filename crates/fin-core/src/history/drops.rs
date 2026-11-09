//! Durable rejections (sub-project 5 spec §8.3; plan 5E Decisions 1–3): what the `drop`
//! records on a branch's lineage leave dropped for it.
//!
//! An origin root is **dropped for T** when the newest `drop` record on T's lineage that names
//! it lists it in `drops` (not `undrops`). "Newest" is lineage order: an ancestor's records up
//! to the fork seq come before the branch's own, and records on one branch go by seq. So a
//! branch forked after a record inherits it, and a record written on the parent after the fork
//! never reaches the child, as with every record. The answer comes from the records alone, so
//! a writer, a follower that imported them in another order and a rebuild agree.
//!
//! Honoured by merge candidate selection ([`super::candidates`]: `DROPPED`) and the merged
//! state ([`super::left_to_merge`]); not by cherry-pick (an explicit pick is a deliberate act)
//! and not by the undo stacks (a `drop` is no unit).

use std::collections::{BTreeMap, BTreeSet};

use serde_json::json;

use super::merge::TO_HEAD;
use super::origin::Roots;
use super::{Lineage, LogView};
use crate::error::CoreError;
use crate::record::{Link, RecordKind};

/// One origin a `drop` record names: on `branch` at `seq`, `dropped` for `drops`, `false` for
/// `undrops`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DropMark {
    pub branch: String,
    pub seq: u64,
    pub origin: String,
    pub dropped: bool,
}

/// Every [`DropMark`] of the `drop` records on `lineage`, in lineage order (oldest first: the
/// furthest ancestor's records up to its fork seq, … , then the branch's own by seq; within a
/// record its `drops`, then its `undrops`). Reads the records ([`LogView::records`]): the
/// default of [`LogView::drops_on`], which a store answers from an index instead.
pub fn marks_from_records<L: LogView + ?Sized>(
    log: &L,
    lineage: &Lineage,
) -> Result<Vec<DropMark>, CoreError> {
    let mut out = Vec::new();
    for (branch, after, through) in lineage.segs.iter().rev() {
        let through = through.unwrap_or(TO_HEAD);
        if through <= *after {
            continue;
        }
        for rec in log.records(branch, *after, through)? {
            if rec.record.kind != RecordKind::Drop {
                continue;
            }
            let Link::Drop { drops, undrops, .. } = rec.record.link()? else {
                continue;
            };
            let mark = |origin: String, dropped: bool| DropMark {
                branch: branch.clone(),
                seq: rec.seq,
                origin,
                dropped,
            };
            out.extend(drops.into_iter().map(|o| mark(o, true)));
            out.extend(undrops.into_iter().map(|o| mark(o, false)));
        }
    }
    Ok(out)
}

/// The origin roots dropped for the branch whose lineage is `lineage` (module docs).
pub fn dropped_for(log: &dyn LogView, lineage: &Lineage) -> Result<BTreeSet<String>, CoreError> {
    let mut last: BTreeMap<String, bool> = BTreeMap::new();
    for m in log.drops_on(lineage)? {
        last.insert(m.origin, m.dropped);
    }
    Ok(last
        .into_iter()
        .filter_map(|(origin, dropped)| dropped.then_some(origin))
        .collect())
}

/// The origin roots a `drop` on `into` may name in `drops` for source `from` (plan 5E
/// Decisions 5 and 15: a rejection is of an entry not yet merged), each with whether `into`
/// already holds it. The roots are those of the records a merge of `from` into `into` reads as
/// its source path (`from`'s lineage after its common point with `into`) that a merge could
/// copy (content records and `merge` copies, as [`super::candidates`] reads them); `true` marks
/// one `into` holds ([`Roots::present`]: it originates on `into`'s lineage, or a merge or pick
/// copied it there), which is no drop. Empty when the two are one branch or share no history.
pub fn source_roots(
    log: &dyn LogView,
    from: &str,
    into: &str,
) -> Result<BTreeMap<String, Held>, CoreError> {
    let (source_l, target_l) = (log.lineage(from)?, log.lineage(into)?);
    let c = super::common_point(&source_l, &target_l);
    if c.0.is_empty() || from == into {
        return Ok(BTreeMap::new());
    }
    let source = super::merge::path_after(log, &source_l, &c)?;
    let target = super::merge::path_after(log, &target_l, &c)?;
    let roots = Roots::new(log, &target_l, &source, &target)?;
    let mut out = BTreeMap::new();
    for (i, rec) in source.iter().enumerate() {
        let kind = rec.record.kind;
        if kind.is_content() || kind == RecordKind::Merge {
            let held = if roots.on_target[i] {
                Held::Own
            } else if roots.present[i] {
                Held::Copied
            } else {
                Held::No
            };
            out.insert(roots.source[i].clone(), held);
        }
    }
    Ok(out)
}

/// Whether the target of a drop already holds a source root ([`source_roots`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Held {
    /// Not yet merged: a drop may name it.
    No,
    /// It originates on the target's lineage.
    Own,
    /// A merge or pick copied it to the target.
    Copied,
}

/// Checks the ids of a `drop` record on `into` for source `from` (plan 5E Decisions 5, 15):
/// each of `drops` must be an origin root `from` would still merge into `into`, each of
/// `undrops` an origin dropped for `into` now ([`dropped_for`]), whatever became of it on the
/// source since (a pick, an archive), so a rejection can always be lifted.
///
/// Errors: `INVALID_OP {reason, txId, from, into}` for the first bad id, `reason` being
/// `notOnSource` (not a root `from` brings, or one originating on `into`), `alreadyPresent`
/// (already merged or picked into `into`) or `notDropped` (an undrop of an origin not dropped
/// for `into`); what the log's reads fail with.
pub fn check_drop(
    log: &dyn LogView,
    from: &str,
    into: &str,
    drops: &[String],
    undrops: &[String],
) -> Result<(), CoreError> {
    let refuse = |id: &str, reason: &str, why: String, fix: &str| {
        Err(CoreError::new("INVALID_OP", why)
            .with_details(json!({ "reason": reason, "txId": id, "from": from, "into": into }))
            .with_fix(fix))
    };
    if !drops.is_empty() {
        let roots = source_roots(log, from, into)?;
        for id in drops {
            match roots.get(id) {
                Some(Held::No) => {}
                Some(Held::Copied) => {
                    return refuse(
                        id,
                        "alreadyPresent",
                        format!("transaction {id} is already merged into {into}"),
                        "name the origin txIds of changes not yet merged (revert a merge to undo it)",
                    );
                }
                Some(Held::Own) | None => {
                    return refuse(
                        id,
                        "notOnSource",
                        format!(
                            "transaction {id} is not a change branch {from} would merge into {into}"
                        ),
                        "name the origin txIds of changes the source branch would merge into the target",
                    );
                }
            }
        }
    }
    if !undrops.is_empty() {
        let dropped = dropped_for(log, &log.lineage(into)?)?;
        if let Some(id) = undrops.iter().find(|id| !dropped.contains(*id)) {
            return refuse(
                id,
                "notDropped",
                format!("transaction {id} is not dropped for {into}"),
                "name origin txIds dropped for the target (see the branch's dropped list)",
            );
        }
    }
    Ok(())
}
