//! Rebasing a stale transaction over the transactions committed since its base revision.
//!
//! Positions move with the committed ripples: a position on the same track at or after a shift's
//! `from` moves by its `by` (so of two inserts at one point, the committed one ends up first).
//! Edits of a clip the committed side removed, trims of the same clip edge, and inserts that
//! would land strictly inside a committed clip are conflicts.
//!
//! Within a stale transaction, later ops are written against the project as changed by its
//! earlier ops. The committed shifts are therefore carried forward through each of our inserts
//! and restores (whose effect is known from the op alone). Our trims and deletes move clips by
//! amounts only the project knows, so a later position on a track the committed side rippled is
//! reported as a conflict rather than guessed.

use serde_json::json;

use crate::error::CoreError;
use crate::ops::{self, Op};
use crate::time::Rational;
use crate::tx::{CommittedTx, Diff, Tx};

/// Why an op of a stale transaction cannot be rebased. Converts to a `CONFLICT` `CoreError`.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("CONFLICT: {reason} (op {op_index}, against {against_tx})")]
pub struct Conflict {
    pub reason: String,
    pub op_index: usize,
    pub against_tx: String,
}

impl Conflict {
    /// A conflict whose op index and committed tx id `rebase_tx` fills in.
    pub(crate) fn new(reason: impl Into<String>) -> Self {
        Conflict {
            reason: reason.into(),
            op_index: 0,
            against_tx: String::new(),
        }
    }
}

impl From<Conflict> for CoreError {
    fn from(c: Conflict) -> Self {
        CoreError::new(
            "CONFLICT",
            format!(
                "op {} conflicts with committed tx {}: {}",
                c.op_index, c.against_tx, c.reason
            ),
        )
        .with_details(
            json!({ "reason": c.reason, "opIndex": c.op_index, "againstTx": c.against_tx }),
        )
        .with_fix("read the project at the current head and send the edit again")
    }
}

fn overflow() -> Conflict {
    Conflict::new("time overflow while rebasing")
}

/// `pos` on `track` moved by every shift in `diff` on that track whose `from` is at or before it.
pub(crate) fn shift_position(
    track: &str,
    pos: Rational,
    diff: &Diff,
) -> Result<Rational, Conflict> {
    let mut pos = pos;
    for s in diff.shifts.iter().filter(|s| s.track == track) {
        if pos >= s.from {
            pos = pos.checked_add(s.by).map_err(|_| overflow())?;
        }
    }
    Ok(pos)
}

/// For an op that places a clip, its track and `[start, end)`. The start comes from the op's
/// shift when present, since that shift is carried forward through our own earlier inserts.
pub(crate) fn inserted_range(
    over: &Op,
    over_diff: &Diff,
) -> Result<Option<(String, Rational, Rational)>, Conflict> {
    let (track, start, duration) = match over {
        Op::ClipInsert {
            track,
            at,
            duration,
            ..
        } => (track, *at, *duration),
        Op::ClipRestore { clip, track, .. } => (track, clip.start, clip.duration),
        _ => return Ok(None),
    };
    let start = over_diff
        .shifts
        .first()
        .filter(|s| &s.track == track)
        .map_or(start, |s| s.from);
    let end = start.checked_add(duration).map_err(|_| overflow())?;
    Ok(Some((track.clone(), start, end)))
}

/// The clip an op removes, if any.
pub(crate) fn removed_clip(over: &Op) -> Option<&str> {
    match over {
        Op::ClipDelete { clip } => Some(clip),
        Op::ClipRemove { id, .. } => Some(id),
        _ => None,
    }
}

/// Where an op of ours places a clip and how far it ripples the track: `(track, at, by)`.
fn placement(op: &Op) -> Option<(&str, Rational, Rational)> {
    match op {
        Op::ClipInsert {
            track,
            at,
            duration,
            ..
        } => Some((track, *at, *duration)),
        Op::ClipRestore {
            clip,
            track,
            ripple: true,
        } => Some((track, clip.start, clip.duration)),
        _ => None,
    }
}

/// `tx` with every op rebased over each committed tx with `rev > tx.base_rev` (in rev order),
/// and `base_rev` set to the last of them. Unchanged when nothing newer was committed.
pub fn rebase_tx(tx: &Tx, since: &[CommittedTx]) -> Result<Tx, Conflict> {
    let mut newer: Vec<&CommittedTx> = since.iter().filter(|c| c.rev > tx.base_rev).collect();
    newer.sort_by_key(|c| c.rev);
    let mut out = tx.clone();
    for committed in newer {
        out.ops = rebase_over(&out.ops, committed)?;
        out.base_rev = committed.rev;
    }
    Ok(out)
}

/// Pairs each committed op (placeholders resolved) with its own shift.
fn split(committed: &CommittedTx) -> Result<Vec<(Op, Diff)>, Conflict> {
    let mismatch = || Conflict::new("committed diff does not match its ops");
    let mut shifts = committed.diff.shifts.iter();
    let mut out = Vec::with_capacity(committed.tx.ops.len());
    for op in &committed.tx.ops {
        let mut diff = Diff::default();
        if op.records_shift() {
            diff.shifts
                .push(shifts.next().ok_or_else(mismatch)?.clone());
        }
        out.push((op.resolved(&committed.bindings), diff));
    }
    if shifts.next().is_some() {
        return Err(mismatch());
    }
    Ok(out)
}

fn rebase_over(ops: &[Op], committed: &CommittedTx) -> Result<Vec<Op>, Conflict> {
    let tag = |mut c: Conflict, i: usize| {
        c.op_index = i;
        c.against_tx = committed.tx.tx_id.clone();
        c
    };
    let mut over = split(committed).map_err(|c| tag(c, 0))?;
    let mut blind = false;
    let mut out = Vec::with_capacity(ops.len());
    for (i, op) in ops.iter().enumerate() {
        if blind && let Some(track) = positioned_track(op) {
            let moved = over
                .iter()
                .flat_map(|(_, d)| &d.shifts)
                .any(|s| s.track == track && !s.by.is_zero());
            if moved {
                return Err(tag(
                    Conflict::new("position follows a trim or delete on a track that moved"),
                    i,
                ));
            }
        }
        let mut cur = op.clone();
        for (o, d) in over.iter_mut() {
            let next = ops::rebase(&cur, o, d).map_err(|c| tag(c, i))?;
            // Carry the committed shift past our op, so our later ops see it in their frame.
            if let Some((track, at, by)) = placement(&cur) {
                for s in d
                    .shifts
                    .iter_mut()
                    .filter(|s| s.track == track && s.from > at)
                {
                    s.from = s.from.checked_add(by).map_err(|_| tag(overflow(), i))?;
                }
            }
            cur = next;
        }
        blind |= matches!(
            cur,
            Op::ClipTrim { .. } | Op::ClipDelete { .. } | Op::ClipRemove { .. }
        );
        out.push(cur);
    }
    Ok(out)
}

/// The track of an op that carries a timeline position (which our earlier trims/deletes move).
fn positioned_track(op: &Op) -> Option<&str> {
    match op {
        Op::ClipInsert { track, .. } | Op::ClipRestore { track, .. } => Some(track),
        _ => None,
    }
}
