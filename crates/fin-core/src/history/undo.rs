//! Undo and redo derived from the log (spec §4.5, arch §6.4; plan 4B Decisions 2, 22–25).
//!
//! **Stacks** ([`derive_stacks`]) are never stored. For one author on one branch they come
//! from the branch's own records (seq above its fork), oldest first:
//! - the author's `edit`, `cherryPick` or `revert` record, or the first record of a `merge`
//!   group, pushes a unit on the undo stack (a whole merge group is one unit, its id the
//!   `mergeId`) and clears the redo stack;
//! - the author's `undo` pops the units it `undoes` and pushes the undo record on the redo
//!   stack;
//! - the author's `redo` pops its undo from the redo stack and pushes **the redo record** as a
//!   unit of its own, `source = Redo` (Decision 23);
//! - everything else (other authors, `checkpoint`, `branch`, `branchSet`) is skipped;
//! - last, a unit with a record that was merged away ([`LogView::merged_away`]) drops the undo
//!   stack from that unit down, and an undo record merged away drops the redo stack from it
//!   down (Decision 22; arch §6.4 drops both stacks when the branch is merged).
//!
//! The stacks need only each record's kind, author and links ([`StackRec`]), which a store
//! reads without parsing records ([`LogView::stack_records`]); a plan then reads the unit's
//! records and the records since (plan 4B Task 20).
//!
//! **Planning** an undo takes the top unit's stored inverses (its records newest first, or the
//! redo record's), in the frame right after the unit (the state at its last seq), and rebases
//! them over the branch's records since, mirrors included (an undo inside the interval
//! mirrors its unit, a redo its undo: [`steps_from`]), re-running each op at the head with full
//! policy ([`rebase_apply`], `Internal`: inverses may remove and restore). A redo does the same
//! with the top undo record's inverse. One op per op, in order, so an undo's ops stay its
//! units' inverses, newest unit first, and a later stale rebase can pair them again.

use std::collections::{BTreeSet, HashSet};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::{LogRec, LogView, Planned};
use crate::apply::{ApplyMode, apply_tx_mode};
use crate::error::CoreError;
use crate::ids::{IdAlloc, MemIds};
use crate::model::SCHEMA_VERSION;
use crate::ops::Op;
use crate::rebase::{Conflict, Conflicts, Rebase, rebase_apply, steps_from};
use crate::record::{Link, RecordKind};
use crate::tx::{CommittedTx, Tx};

/// One entry of an undo or redo stack.
///
/// - `id`: what an `undo` names in `undoes` (a record's txId, a merge group's `mergeId`, the
///   redo record's txId for a redo unit), or, on the redo stack, the undo record's txId;
/// - `records`: the txIds of its records, oldest first: whose stored `inverse` an undo (or,
///   for an undo record, a redo) takes;
/// - `last_seq`: the seq of its last record, the frame its inverses apply in.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct UnitEntry {
    pub id: String,
    pub records: Vec<String>,
    pub last_seq: u64,
    /// The record whose stored `inverse` an undo uses: the unit's records, or the redo
    /// record that recreated it (Decision 23).
    pub source: UnitSource,
}

/// Where an undo takes a unit's inverse from ([`UnitEntry::source`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum UnitSource {
    /// The unit's own records.
    Records,
    /// The redo record `tx_id`, which recreated an undone unit.
    Redo { tx_id: String },
}

/// An author's undo and redo stacks on a branch, bottom first (the top is the last entry).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Stacks {
    pub undo: Vec<UnitEntry>,
    pub redo: Vec<UnitEntry>,
}

/// A record as the stacks read it: no intent ops, inverses or diff ([`derive_stacks`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StackRec {
    pub seq: u64,
    pub tx_id: String,
    pub kind: RecordKind,
    /// The intent's author (Decision 25).
    pub author: String,
    pub merge_id: Option<String>,
    /// The units an `undo` names (`undoes`), newest first.
    pub undoes: Vec<String>,
    /// The undo a `redo` reverses.
    pub redoes: Option<String>,
}

impl StackRec {
    pub fn of(rec: &LogRec) -> StackRec {
        let r = &rec.record;
        StackRec {
            seq: rec.seq,
            tx_id: r.tx_id.clone(),
            kind: r.kind,
            author: r.intent.author.clone(),
            merge_id: r.merge_id.clone(),
            undoes: r.undoes.clone().unwrap_or_default(),
            redoes: r.redoes.clone(),
        }
    }
}

/// The stacks of `author` from a branch's own records `own` (oldest first), dropping from the
/// top-most unit with a record in `merged_away` down (module docs).
pub fn derive_stacks(own: &[StackRec], author: &str, merged_away: &HashSet<String>) -> Stacks {
    let mut s = Stacks::default();
    // The merge group the top unit is, while its records follow one another.
    let mut group: Option<&str> = None;
    for r in own {
        if r.kind == RecordKind::Merge
            && let (Some(g), Some(m), Some(top)) = (group, r.merge_id.as_deref(), s.undo.last_mut())
            && g == m
        {
            top.records.push(r.tx_id.clone());
            top.last_seq = r.seq;
            continue;
        }
        group = None;
        if r.author != author {
            continue;
        }
        let unit = |id: &str, source: UnitSource| UnitEntry {
            id: id.to_string(),
            records: vec![r.tx_id.clone()],
            last_seq: r.seq,
            source,
        };
        match r.kind {
            RecordKind::Edit | RecordKind::CherryPick | RecordKind::Revert => {
                s.undo.push(unit(&r.tx_id, UnitSource::Records));
                s.redo.clear();
            }
            RecordKind::Merge => {
                let id = r.merge_id.as_deref().unwrap_or(&r.tx_id);
                s.undo.push(unit(id, UnitSource::Records));
                s.redo.clear();
                group = r.merge_id.as_deref();
            }
            RecordKind::Undo => {
                for u in &r.undoes {
                    if let Some(i) = s.undo.iter().rposition(|e| e.id == *u) {
                        s.undo.remove(i);
                    }
                }
                s.redo.push(unit(&r.tx_id, UnitSource::Records));
            }
            RecordKind::Redo => {
                if let Some(i) = r
                    .redoes
                    .as_deref()
                    .and_then(|u| s.redo.iter().rposition(|e| e.id == u))
                {
                    s.redo.remove(i);
                }
                let source = UnitSource::Redo {
                    tx_id: r.tx_id.clone(),
                };
                s.undo.push(unit(&r.tx_id, source));
            }
            _ => {}
        }
    }
    // Merged away: each stack drops from its top-most entry with such a record down (an
    // entry on the redo stack is an undo record, merged with the rest).
    for stack in [&mut s.undo, &mut s.redo] {
        if let Some(i) = stack
            .iter()
            .rposition(|u| u.records.iter().any(|t| merged_away.contains(t)))
        {
            stack.drain(..=i);
        }
    }
    s
}

/// What a planner reads first: the branch's fork and head, and the author's stacks.
pub(super) struct Read {
    pub(super) fork: u64,
    pub(super) head_seq: u64,
    pub(super) head: crate::model::Project,
    pub(super) stacks: Stacks,
}

pub(super) fn read(log: &dyn LogView, branch: &str, author: &str) -> Result<Read, CoreError> {
    let fork = log.lineage(branch)?.fork_seq();
    let (head_seq, head) = log.head(branch)?;
    let own = log.stack_records(branch, fork, head_seq)?;
    let stacks = derive_stacks(&own, author, &log.merged_away(branch)?);
    Ok(Read {
        fork,
        head_seq,
        head,
        stacks,
    })
}

/// The `undo` record of `author`'s top unit on `branch` (module docs): `Undo { undoes: [id] }`
/// with the unit's inverses rebased to the head. `NOTHING_TO_UNDO` when the undo stack is
/// empty; `CONFLICT` (`details.conflicts`) when an op cannot be carried to the head.
pub fn plan_undo(
    log: &dyn LogView,
    branch: &str,
    author: &str,
    tx_id: &str,
    ids: &mut dyn IdAlloc,
) -> Result<Planned, CoreError> {
    let r = read(log, branch, author)?;
    let Some(unit) = r.stacks.undo.last() else {
        return Err(nothing("NOTHING_TO_UNDO", "undo", branch, author));
    };
    let sources = match &unit.source {
        UnitSource::Records => unit.records.clone(),
        UnitSource::Redo { tx_id } => vec![tx_id.clone()],
    };
    // The unit's records end at its last seq, one after another (a merge group's included),
    // and the records since follow them.
    let first = unit.last_seq.saturating_sub(sources.len() as u64);
    let read = log.records(branch, first.max(r.fork), r.head_seq)?;
    let mut ops = Vec::new();
    let mut recs = Vec::new();
    for id in sources.iter().rev() {
        let rec = own_record(&read, id)?;
        ops.extend(rec.record.inverse.iter().cloned());
        recs.push(rec);
    }
    let absorbs = super::absorbs::of_inverses(log, &recs)?;
    let link = Link::Undo {
        undoes: vec![unit.id.clone()],
    };
    let tx = intent(tx_id, author, branch, unit.last_seq, ops);
    rebase_inverse(log, &r, &read, unit.last_seq, (tx, &absorbs), link, ids)
}

/// The `redo` record of `author`'s top undo on `branch`: `Redo { redoes: <undo txId> }` with
/// that undo record's inverse rebased to the head. `NOTHING_TO_REDO` when the redo stack is
/// empty (a new edit by the author clears it); `CONFLICT` as for [`plan_undo`].
pub fn plan_redo(
    log: &dyn LogView,
    branch: &str,
    author: &str,
    tx_id: &str,
    ids: &mut dyn IdAlloc,
) -> Result<Planned, CoreError> {
    let r = read(log, branch, author)?;
    let Some(undo) = r.stacks.redo.last() else {
        return Err(nothing("NOTHING_TO_REDO", "redo", branch, author));
    };
    let read = log.records(branch, undo.last_seq.saturating_sub(1), r.head_seq)?;
    let rec = own_record(&read, &undo.id)?;
    let ops = rec.record.inverse.clone();
    let absorbs = super::absorbs::of_inverses(log, &[rec])?;
    let link = Link::Redo {
        redoes: undo.id.clone(),
    };
    let tx = intent(tx_id, author, branch, undo.last_seq, ops);
    rebase_inverse(log, &r, &read, undo.last_seq, (tx, &absorbs), link, ids)
}

pub(super) fn intent(tx_id: &str, author: &str, branch: &str, at: u64, ops: Vec<Op>) -> Tx {
    Tx {
        tx_id: tx_id.to_string(),
        author: author.to_string(),
        branch: branch.to_string(),
        base_rev: at,
        schema_version: SCHEMA_VERSION,
        ops,
        args_hash: None,
    }
}

/// `tx` (inverses written for the state after seq `at`, with its split-undo removals: see
/// `Rebase::absorbs`) rebased over the branch's own records after `at` (`read` holds them,
/// oldest first) and applied at the head.
fn rebase_inverse(
    log: &dyn LogView,
    r: &Read,
    read: &[LogRec],
    at: u64,
    tx: (Tx, &BTreeSet<usize>),
    link: Link,
    ids: &mut dyn IdAlloc,
) -> Result<Planned, CoreError> {
    let k = read.partition_point(|c| c.seq <= at);
    rebase_over(log, (r.head_seq, &r.head), &read[k..], tx, link, ids)
}

/// `tx` (inverses written for the state after `tx.base_rev` on `tx.branch`) rebased over
/// `since` (the branch's records after that seq, oldest first, with their mirrors) and applied
/// at `head`, in `Internal` mode. `CONFLICT` with the one conflict when an op cannot be
/// carried there.
pub(super) fn rebase_over(
    log: &dyn LogView,
    head: (u64, &crate::model::Project),
    since: &[LogRec],
    tx: (Tx, &BTreeSet<usize>),
    link: Link,
    ids: &mut dyn IdAlloc,
) -> Result<Planned, CoreError> {
    try_rebase_over(log, head, since, tx, link, ids)?
        .map_err(|c| CoreError::from(Conflicts::one(c)))
}

/// [`rebase_over`], with the conflict as a value (`Ok(Err(c))`); `Err` when the log's reads
/// fail or `tx` does not apply in its own frame.
pub(super) fn try_rebase_over(
    log: &dyn LogView,
    (head_seq, head): (u64, &crate::model::Project),
    since: &[LogRec],
    (tx, absorbs): (Tx, &BTreeSet<usize>),
    link: Link,
    ids: &mut dyn IdAlloc,
) -> Result<Result<Planned, Conflict>, CoreError> {
    let frame = log.state_at(&tx.branch, tx.base_rev)?;
    // The inverses' own maps, in their frame (they name literal ids: nothing is allocated).
    let own = apply_tx_mode(&frame, &tx, &mut MemIds::after(&frame), ApplyMode::Internal)?;
    let since: Vec<CommittedTx> = since
        .iter()
        .map(LogRec::committed)
        .collect::<Result<_, _>>()?;
    let steps = steps_from(&since.iter().collect::<Vec<_>>())?;
    let places = crate::rebase::places::places(&frame, &tx, ApplyMode::Internal)?;
    Ok(rebase_apply(
        head,
        &Rebase::new(&tx, &own.diff, &steps)
            .with_env(super::env_of(log))
            .with_absorbs(absorbs)
            .with_places(&places),
        ids,
        ApplyMode::Internal,
    )
    .map(|rebased| Planned {
        link,
        intent: Tx {
            base_rev: head_seq,
            ..rebased.tx
        },
        applied: rebased.applied,
    }))
}

/// The own record `tx_id` (a unit's record): one the stacks were derived from.
pub(super) fn own_record<'a>(own: &'a [LogRec], tx_id: &str) -> Result<&'a LogRec, CoreError> {
    own.iter().find(|c| c.record.tx_id == tx_id).ok_or_else(|| {
        CoreError::new(
            "DB_ERROR",
            format!("the undo stacks name record {tx_id}, which the branch does not hold"),
        )
        .with_details(json!({ "txId": tx_id }))
    })
}

pub(super) fn nothing(code: &'static str, what: &str, branch: &str, author: &str) -> CoreError {
    CoreError::new(
        code,
        format!("{author} has nothing to {what} on branch {branch}"),
    )
    .with_details(json!({ "branch": branch, "author": author }))
    .with_fix(match what {
        "undo" => {
            "undo takes the author's own edits made on this branch since its fork, newest first; \
             check the author ($FIN_AUTHOR) and the branch (--branch)"
        }
        _ => "redo follows an undo by the same author; a new edit by that author clears it",
    })
}
