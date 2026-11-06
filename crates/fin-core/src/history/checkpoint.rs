//! `undo --to <checkpoint>` (spec §4.5; plan 4B Decisions 2 and 22, Task 15): one `undo`
//! record that takes back every unit of one author made on the branch after a checkpoint.
//!
//! **Which units:** from the top of the author's undo stack ([`derive_stacks`]) down, while
//! the unit was made after the checkpoint's seq. A unit of records was made at its last seq.
//! A redo unit stands for the units it recreated (Decision 23), so it was made when they were
//! (the newest of them): a redo of a unit made before the checkpoint stops the walk, as that
//! unit is part of the checkpoint's state. A checkpoint at or before the branch's fork takes
//! every unit of the author's on the branch (Decision 22).
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
use crate::rebase::{Conflict, Conflicts, Rebase, Rebased, rebase_apply, steps_from};
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
    let units = after_checkpoint(&r.own, &r.stacks.undo, checkpoint_seq);
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

/// The units `undo --to` takes, newest first (module docs).
fn after_checkpoint<'a>(
    own: &[LogRec],
    undo: &'a [UnitEntry],
    checkpoint_seq: u64,
) -> Vec<&'a UnitEntry> {
    let by_id: HashMap<&str, &LogRec> = own.iter().map(|c| (c.record.tx_id.as_str(), c)).collect();
    undo.iter()
        .rev()
        .take_while(|u| made_at(own, &by_id, u) > checkpoint_seq)
        .collect()
}

/// When `unit`'s content was made: its last seq, or for a redo unit the newest of the units
/// its undo had taken back (recursively, through earlier redos).
fn made_at(own: &[LogRec], by_id: &HashMap<&str, &LogRec>, unit: &UnitEntry) -> u64 {
    match &unit.source {
        UnitSource::Records => unit.last_seq,
        UnitSource::Redo { tx_id } => redo_made_at(own, by_id, tx_id).unwrap_or(unit.last_seq),
    }
}

fn redo_made_at(own: &[LogRec], by_id: &HashMap<&str, &LogRec>, redo: &str) -> Option<u64> {
    let r = by_id.get(redo)?;
    let undo = by_id.get(r.record.redoes.as_deref()?)?;
    let mut newest = None;
    for id in undo.record.undoes.iter().flatten() {
        let at = match by_id.get(id.as_str()) {
            // A redo unit again: it names a record older than this one (seqs only go down).
            Some(c) if c.record.kind == RecordKind::Redo && c.seq < r.seq => {
                redo_made_at(own, by_id, id).unwrap_or(c.seq)
            }
            Some(c) => c.seq,
            // A merge group's id: the group's last record.
            None => own
                .iter()
                .filter(|c| c.record.merge_id.as_deref() == Some(id.as_str()))
                .map(|c| c.seq)
                .max()?,
        };
        newest = newest.max(Some(at));
    }
    newest
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
