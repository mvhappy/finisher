//! `undo --to <checkpoint>` (spec §4.5; plan 4B Decisions 2 and 22, Task 15): one `undo`
//! record that takes back every unit of one author made on the branch after a checkpoint.
//!
//! **Which units:** from the top of the author's undo stack ([`derive_stacks`]) down, while
//! the unit was made after the checkpoint's seq. A unit of records was made at its last seq.
//! A redo unit stands for the units its undo took back (Decision 23), so it was made when
//! they were, recursively through earlier redos: a redo of a unit made before the checkpoint
//! stops the walk, as that unit is part of the checkpoint's state. But when that undo is at
//! or before the checkpoint, the checkpoint's state lacks those units, and the redo was made
//! at its own seq (it is taken). A redo that stands for units on both sides of the checkpoint
//! (a redo of an earlier `undo --to`) can be neither taken nor kept without changing the
//! checkpoint's state: `CONFLICT` with reason `STRADDLES_CHECKPOINT`, all or nothing. A
//! checkpoint at or before the branch's fork takes every unit of the author's on the branch
//! (Decision 22).
//!
//! **Planning** is that many undos in a row, written as one record: newest unit first, each
//! unit's stored inverse is rebased over the branch's records after it and over the inverses
//! already planned in this record, each of those mirroring its unit ([`steps_from`]), and
//! re-run at the evolving head with full policy. The record's ops are the units' inverses,
//! newest unit first, one op per op, so a later stale rebase cuts it into one part per unit
//! and pairs each with its unit again; one redo reverses it whole.
//!
//! **All or nothing:** a unit whose inverse cannot be carried to the head is a conflict; the
//! planning goes on as if that unit were left alone (a conflict an older unit then meets in
//! its records is `DEPENDS_ON_SKIPPED`), so the `CONFLICT` lists every unit that fails, and
//! nothing is written.
//!
//! [`steps_from`]: crate::rebase::steps_from

use std::collections::{HashMap, HashSet};

use serde_json::{Value, json};

use super::merge::on_skipped;
use super::undo::{UnitEntry, UnitSource, intent, nothing, own_record, read};
use super::{LogRec, LogView, Planned};
use crate::apply::{ApplyMode, apply_tx_mode};
use crate::error::CoreError;
use crate::ids::{IdAlloc, MemIds};
use crate::model::Project;
use crate::ops::Op;
use crate::rebase::{Conflict, Conflicts, Rebase, Rebased, reasons, rebase_apply, steps_from};
use crate::record::{Link, RecordKind};
use crate::tx::{CommittedTx, Tx};

/// The `undo` record that takes back `author`'s units on `branch` made after the checkpoint
/// at `checkpoint_seq` (module docs): `Undo { undoes }` naming them newest first, its ops
/// their inverses rebased to the head. `NOTHING_TO_UNDO` when there is none (`details`
/// `checkpointSeq` too); `CONFLICT` with every unit that cannot be carried to the head
/// (`details.conflicts`; each `opIndex` is the op's index in the whole record, and
/// `details.unit` names the unit).
pub fn plan_undo_to(
    log: &dyn LogView,
    branch: &str,
    author: &str,
    checkpoint_seq: u64,
    tx_id: &str,
    ids: &mut dyn IdAlloc,
) -> Result<Planned, CoreError> {
    let r = read(log, branch, author)?;
    let units = after_checkpoint(&r.own, &r.stacks.undo, checkpoint_seq, tx_id)?;
    if units.is_empty() {
        let mut e = nothing("NOTHING_TO_UNDO", "undo", branch, author);
        e.message =
            format!("{author} has nothing to undo on branch {branch} after seq {checkpoint_seq}");
        if let Value::Object(d) = &mut e.details {
            d.insert("checkpointSeq".into(), json!(checkpoint_seq));
        }
        return Err(e);
    }
    let (mut seq, mut head) = (r.head_seq, r.head.clone());
    // The undos planned so far, as the records they would be: later units' steps.
    let mut planned: Vec<CommittedTx> = Vec::new();
    let (mut ops, mut undoes, mut conflicts) = (Vec::new(), Vec::new(), Vec::new());
    let mut skipped: HashSet<String> = HashSet::new();
    let mut offset = 0;
    for unit in units {
        let inverse = unit_inverse(&r.own, unit)?;
        let n = inverse.len();
        let tx = intent(tx_id, author, branch, unit.last_seq, inverse);
        let k = r.own.partition_point(|c| c.seq <= unit.last_seq);
        let mut since: Vec<CommittedTx> = r.own[k..]
            .iter()
            .map(LogRec::committed)
            .collect::<Result<_, _>>()?;
        since.extend(planned.iter().cloned());
        // The inverses name literal ids; a scratch allocator keeps `ids` for the final apply.
        let mut scratch = MemIds::after(&head);
        match rebase_steps(log, &head, &since, &tx, &mut scratch)? {
            Ok(rebased) => {
                seq += 1;
                ops.extend(rebased.tx.ops.iter().cloned());
                planned.push(CommittedTx {
                    rev: seq,
                    tx: Tx {
                        base_rev: seq - 1,
                        ..rebased.tx
                    },
                    diff: rebased.applied.diff,
                    bindings: rebased.applied.bindings,
                    link: Some(Box::new(Link::Undo {
                        undoes: vec![unit.id.clone()],
                    })),
                });
                head = rebased.applied.project;
            }
            Err(c) => {
                let skipped_refs = skipped.iter().map(String::as_str).collect();
                conflicts.push(in_record(on_skipped(c, &skipped_refs), offset, &unit.id));
                skipped.extend(unit_sources(unit));
            }
        }
        undoes.push(unit.id.clone());
        offset += n;
    }
    if let Some(all) = Conflicts::new(conflicts) {
        return Err(all.into());
    }
    // One record: the planned ops in a row, at the head (as the undos in a row left it).
    let tx = Tx {
        base_rev: r.head_seq,
        ..intent(tx_id, author, branch, r.head_seq, ops)
    };
    let applied = apply_tx_mode(&r.head, &tx, ids, ApplyMode::Internal)?;
    debug_assert_eq!(applied.project, head, "one record is the undos in a row");
    Ok(Planned {
        link: Link::Undo { undoes },
        intent: tx,
        applied,
    })
}

/// The records whose stored inverses undo `unit`: its own, or the redo record that recreated
/// it (Decision 23), oldest first.
fn unit_sources(unit: &UnitEntry) -> Vec<String> {
    match &unit.source {
        UnitSource::Records => unit.records.clone(),
        UnitSource::Redo { tx_id } => vec![tx_id.clone()],
    }
}

/// The ops that undo `unit` in the frame right after it: its sources' stored inverses, newest
/// record first (as `plan_undo` takes them).
fn unit_inverse(own: &[LogRec], unit: &UnitEntry) -> Result<Vec<Op>, CoreError> {
    let mut ops = Vec::new();
    for id in unit_sources(unit).iter().rev() {
        ops.extend(own_record(own, id)?.record.inverse.iter().cloned());
    }
    Ok(ops)
}

/// `tx` (inverses written for the state after `tx.base_rev` on `tx.branch`) rebased over
/// `since` (oldest first, their mirrors wired by [`steps_from`]) and re-run at `head` in
/// `Internal` mode. The outer error is a read or a broken record; the inner one the op that
/// cannot be carried to the head.
fn rebase_steps(
    log: &dyn LogView,
    head: &Project,
    since: &[CommittedTx],
    tx: &Tx,
    ids: &mut dyn IdAlloc,
) -> Result<Result<Rebased, Conflict>, CoreError> {
    let frame = log.state_at(&tx.branch, tx.base_rev)?;
    // The inverses' own maps, in their frame (they name literal ids: nothing is allocated).
    let own = apply_tx_mode(&frame, tx, &mut MemIds::after(&frame), ApplyMode::Internal)?;
    let steps = steps_from(&since.iter().collect::<Vec<_>>())?;
    Ok(rebase_apply(
        head,
        &Rebase::new(tx, &own.diff, &steps),
        ids,
        ApplyMode::Internal,
    ))
}

/// The units `undo --to` takes, newest first (module docs); `CONFLICT`
/// (`STRADDLES_CHECKPOINT`) at a redo unit that stands for units on both sides of it.
fn after_checkpoint<'a>(
    own: &[LogRec],
    undo: &'a [UnitEntry],
    checkpoint_seq: u64,
    tx_id: &str,
) -> Result<Vec<&'a UnitEntry>, CoreError> {
    let by_id: HashMap<&str, &LogRec> = own.iter().map(|c| (c.record.tx_id.as_str(), c)).collect();
    let made = Made {
        own,
        by_id,
        checkpoint_seq,
    };
    let (mut taken, mut offset) = (Vec::new(), 0);
    for u in undo.iter().rev() {
        let (oldest, newest) = made.unit(u);
        if oldest > checkpoint_seq {
            offset += unit_inverse(own, u)?.len();
            taken.push(u);
        } else if newest <= checkpoint_seq {
            break;
        } else {
            let against = match &u.source {
                UnitSource::Redo { tx_id } => tx_id.clone(),
                UnitSource::Records => u.id.clone(),
            };
            let c = Conflict::new(tx_id, offset, reasons::STRADDLES_CHECKPOINT, Some(against));
            let mut c = in_record(c, 0, &u.id);
            if let Some(Value::Object(d)) = c.details.as_deref_mut() {
                d.insert("checkpointSeq".into(), json!(checkpoint_seq));
            }
            return Err(Conflicts::one(c).into());
        }
    }
    Ok(taken)
}

/// When units were made, judged against one checkpoint (module docs).
struct Made<'a> {
    own: &'a [LogRec],
    by_id: HashMap<&'a str, &'a LogRec>,
    checkpoint_seq: u64,
}

impl Made<'_> {
    /// The oldest and newest seq at which `unit`'s content was made.
    fn unit(&self, unit: &UnitEntry) -> (u64, u64) {
        let own = (unit.last_seq, unit.last_seq);
        match &unit.source {
            UnitSource::Records => own,
            UnitSource::Redo { tx_id } => self.redo(tx_id).unwrap_or(own),
        }
    }

    /// The oldest and newest seq at which the units redo record `redo` recreated were made:
    /// the redo's own seq when its undo is at or before the checkpoint (the checkpoint's
    /// state lacks them), else theirs, recursively through earlier redos.
    fn redo(&self, redo: &str) -> Option<(u64, u64)> {
        let r = self.by_id.get(redo)?;
        let undo = self.by_id.get(r.record.redoes.as_deref()?)?;
        if undo.seq <= self.checkpoint_seq {
            return Some((r.seq, r.seq));
        }
        let mut range: Option<(u64, u64)> = None;
        for id in undo.record.undoes.iter().flatten() {
            let (lo, hi) = match self.by_id.get(id.as_str()) {
                // A redo unit again: it names a record older than this one (seqs only go
                // down).
                Some(c) if c.record.kind == RecordKind::Redo && c.seq < r.seq => {
                    self.redo(id).unwrap_or((c.seq, c.seq))
                }
                Some(c) => (c.seq, c.seq),
                // A merge group's id: the group's last record.
                None => {
                    let last = self
                        .own
                        .iter()
                        .filter(|c| c.record.merge_id.as_deref() == Some(id.as_str()))
                        .map(|c| c.seq)
                        .max()?;
                    (last, last)
                }
            };
            range = Some(range.map_or((lo, hi), |(a, b)| (a.min(lo), b.max(hi))));
        }
        range
    }
}

/// `c` (a conflict of one unit's inverse) as a conflict of the whole record: its op index
/// counted from the record's first op, and `details.unit` naming the unit.
fn in_record(mut c: Conflict, offset: usize, unit: &str) -> Conflict {
    c.op_index += offset;
    let mut d = match c.details.take().map(|b| *b) {
        Some(Value::Object(m)) => m,
        _ => serde_json::Map::new(),
    };
    d.insert("unit".into(), json!(unit));
    c.details = Some(Box::new(Value::Object(d)));
    c
}
