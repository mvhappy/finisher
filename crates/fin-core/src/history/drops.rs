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

use super::merge::TO_HEAD;
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
