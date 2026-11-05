//! Conflict footprints: what one op touches, per track and per entity (spec §4.3).
//!
//! A footprint comes from an op plus its own diff, never from a project, so a committed record
//! is judged by what it stored. Each op module builds its own (`ops::<op>::footprint`);
//! [`footprint`] dispatches, and [`split_diff`] / [`committed_footprints`] cut a record's diff
//! into its ops' own diffs.
//!
//! Rules ([`entity_conflict`]), judged against the intervening records the caller passes (the
//! unmirrored ones, Decision 24):
//! - ours touches or uses X, and the last intervening record that removed or created X removed
//!   it → [`reasons::REMOVED`] (a stale insert of a removed asset included);
//! - the same [`Edge`] of X → [`reasons::SAME_EDGE`]; the same parameter → [`reasons::SAME_PARAM`];
//! - an entity an earlier op of our own transaction created is ours: it is never judged. At a
//!   stale base our placeholders bind the ids the intervening records allocated, and those name
//!   other entities; the re-run at the head decides whether the creation itself still fits.
//!
//! Entity rules ignore tracks: an entity lives on one track unless it moved (4C), and a move
//! must still meet the edits made on the other track. So an edit on track B never conflicts
//! with edits on track A by entity unless they share an entity; `tracks` is for the track-level
//! rules (link groups and moves between tracks, 4C).
//!
//! `REMOVED` wins over `SAME_EDGE`/`SAME_PARAM`; within one reason the earliest record is named.

use std::collections::BTreeSet;

use crate::error::CoreError;
use crate::ops::{self, Edge, Op};
use crate::record::{invalid_format, resolve_ops};
use crate::tx::{CommittedTx, Diff};

use super::conflict::reasons;

/// How an op touches an entity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Touch {
    /// Needs it to exist (and changes it as a whole, e.g. removes it).
    Exists,
    /// Moves one edge of it.
    Edge(Edge),
    /// Sets one parameter of it (4C).
    Param(String),
}

/// What an op touches. `tracks`: the tracks it changes or places on (4C: every track its link
/// groups span, and both tracks of a move between tracks); `touches`: entities it changes;
/// `removes`/`creates`: entities it takes out or brings in; `uses`: entities it refers to
/// without changing them (an insert's asset and track).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Footprint {
    pub tracks: BTreeSet<String>,
    pub touches: Vec<(String, Touch)>,
    pub removes: Vec<String>,
    pub creates: Vec<String>,
    pub uses: Vec<String>,
}

impl Footprint {
    /// An empty footprint on `track` (if any) and every track `diff` shifts.
    pub(crate) fn on(track: Option<&str>, diff: &Diff) -> Footprint {
        let mut tracks: BTreeSet<String> = diff.shifts.iter().map(|s| s.track.clone()).collect();
        tracks.extend(track.map(str::to_string));
        Footprint {
            tracks,
            ..Footprint::default()
        }
    }
}

/// The footprint of `op` from its own diff (`op_diff`: its single shift, if any). A committed
/// op is passed with its placeholders resolved; one of ours may keep its `$name`s.
pub fn footprint(op: &Op, op_diff: &Diff) -> Footprint {
    ops::footprint(op, op_diff)
}

/// Each op's own diff out of the diff of the transaction `ops` made: clip ops record exactly
/// one shift each, asset ops none. Only `shifts` are split; `changes` stay empty.
/// `INVALID_FORMAT` when the shifts do not match the ops.
pub fn split_diff(ops: &[Op], diff: &Diff) -> Result<Vec<Diff>, CoreError> {
    let mismatch = || invalid_format("diff", "its shifts do not match its ops");
    let mut shifts = diff.shifts.iter();
    let mut out = Vec::with_capacity(ops.len());
    for op in ops {
        let mut own = Diff::default();
        if op.records_shift() {
            own.shifts.push(shifts.next().ok_or_else(mismatch)?.clone());
        }
        out.push(own);
    }
    if shifts.next().is_some() {
        return Err(mismatch());
    }
    Ok(out)
}

/// A committed record's ops, placeholders resolved, each with its own diff.
pub fn committed_ops(c: &CommittedTx) -> Result<Vec<(Op, Diff)>, CoreError> {
    let ops = resolve_ops(&c.tx.ops, &c.bindings)?;
    let diffs = split_diff(&c.tx.ops, &c.diff)?;
    Ok(ops.into_iter().zip(diffs).collect())
}

/// The footprints of a committed record's ops, in op order.
pub fn committed_footprints(c: &CommittedTx) -> Result<Vec<Footprint>, CoreError> {
    Ok(committed_ops(c)?
        .iter()
        .map(|(op, d)| footprint(op, d))
        .collect())
}

/// The first entity conflict of `ours` with `theirs` (intervening records in order, as
/// `(txId, footprint)`): its reason code and the txId to blame. `None` when there is none.
/// `own` holds the entities earlier ops of our transaction created (their `creates`, as
/// placeholders or ids); those are never judged.
pub fn entity_conflict(
    ours: &Footprint,
    own: &BTreeSet<String>,
    theirs: &[(&str, Footprint)],
) -> Option<(&'static str, String)> {
    // Entities ours needs, each with the record that removed it last (None once re-created).
    let mut needed: Vec<(&str, Option<usize>)> = ours
        .touches
        .iter()
        .map(|(x, _)| x.as_str())
        .chain(ours.uses.iter().map(String::as_str))
        .filter(|x| !own.contains(*x))
        .map(|x| (x, None))
        .collect();
    needed.sort_unstable();
    needed.dedup();
    let mut same: Option<(&'static str, usize)> = None;
    for (i, (_, f)) in theirs.iter().enumerate() {
        for (x, removed_by) in needed.iter_mut() {
            if f.creates.iter().any(|c| c == x) {
                *removed_by = None;
            }
            if f.removes.iter().any(|r| r == x) {
                *removed_by = Some(i);
            }
        }
        if same.is_none() {
            same = same_touch(ours, own, f).map(|reason| (reason, i));
        }
    }
    // The earliest record whose removal still stands.
    let removed = needed.iter().filter_map(|(_, by)| *by).min();
    removed
        .map(|i| (reasons::REMOVED, i))
        .or(same)
        .map(|(reason, i)| (reason, theirs[i].0.to_string()))
}

/// `SAME_EDGE` or `SAME_PARAM` when both touch the same edge or parameter of one entity.
fn same_touch(
    ours: &Footprint,
    own: &BTreeSet<String>,
    theirs: &Footprint,
) -> Option<&'static str> {
    ours.touches.iter().find_map(|(x, t)| {
        let reason = match t {
            Touch::Exists => return None,
            Touch::Edge(_) => reasons::SAME_EDGE,
            Touch::Param(_) => reasons::SAME_PARAM,
        };
        (!own.contains(x) && theirs.touches.iter().any(|(y, u)| y == x && u == t)).then_some(reason)
    })
}
