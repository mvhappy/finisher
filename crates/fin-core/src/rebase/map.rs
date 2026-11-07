//! Exact per-track shift maps and mirrored mappings (ProseMirror `StepMap` / `Mapping`
//! semantics, Decision 16).
//!
//! A [`StepMap`] is one record's `Diff.shifts`, in op order, each shift in the frame left by the
//! previous one. `Shift { from, by }` with `by > 0` inserts `[from, from + by)`; with `by < 0` it
//! removes `[from + by, from)`. Its inverse is `{ from + by, -by }`.
//!
//! A position in a removed range is collapsed to the range start and reported as deleted, with
//! its offset into the range, so a [`Mapping`] can recover it through a mirror: a later map that
//! re-inserts the same range (the rebased copy of the step whose inverse removed it).
//!
//! Without a mirror, only a position strictly inside a removed range is lost. One at an edge
//! (the start with `After`, the end with `Before`) is a boundary, not a reference to the
//! removed content, so it carries on from the range start, as ProseMirror's
//! `deletedAfter` / `deletedBefore` do. The assoc-side edge (the end with `After`, the start
//! with `Before`) is never deleted at all.

use std::sync::Arc;

use crate::error::CoreError;
use crate::time::Rational;
use crate::tx::{Diff, Shift};

/// Which side a position sticks to when content is inserted at it, or which edge of a removed
/// range survives the removal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Assoc {
    Before,
    After,
}

/// One step's shifts, in op order; each is in the frame after the previous one; and its
/// content offsets, in change order. Cloning is cheap (both are shared), so one map serves
/// every rebase that passes through it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct StepMap {
    shifts: Arc<[Shift]>,
    content: Arc<[(String, Rational)]>,
}

/// Where a deleted position was: `offset` into the range removed by shift `shift`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Recover {
    pub shift: usize,
    pub offset: Rational,
}

/// A position mapped through a [`StepMap`]. `deleted` is the first shift that deleted it,
/// edges included (a mirror can recover those); `across` is set when some shift removed a range
/// with the position strictly inside it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MapResult {
    pub pos: Rational,
    pub deleted: Option<Recover>,
    pub across: bool,
}

impl StepMap {
    /// A map of `shifts` with no content offsets.
    pub fn from_shifts(shifts: Vec<Shift>) -> StepMap {
        StepMap::from_parts(shifts, Vec::new())
    }

    /// A map of `shifts` and content offsets `(item, by)`.
    pub fn from_parts(shifts: Vec<Shift>, content: Vec<(String, Rational)>) -> StepMap {
        StepMap {
            shifts: shifts.into(),
            content: content.into(),
        }
    }

    /// The diff's shifts, and the `by` of each of its changes that has one.
    pub fn from_diff(diff: &Diff) -> StepMap {
        StepMap::from_parts(diff.shifts.clone(), content_of(diff))
    }

    pub fn shifts(&self) -> &[Shift] {
        &self.shifts
    }

    /// The content offsets `(item, by)`, in change order.
    pub fn content(&self) -> &[(String, Rational)] {
        &self.content
    }

    /// The sum of the content offsets of `item`.
    fn content_of(&self, item: &str) -> Result<Rational, CoreError> {
        self.content
            .iter()
            .filter(|(id, _)| id == item)
            .try_fold(Rational::zero(), |sum, (_, by)| sum.checked_add(*by))
    }

    /// The inverse map: shifts in reverse order, each `{from, by}` → `{from + by, -by}`;
    /// content offsets in reverse order, each negated.
    pub fn invert(&self) -> Result<StepMap, CoreError> {
        let shifts = self
            .shifts
            .iter()
            .rev()
            .map(|s| {
                Ok(Shift {
                    track: s.track.clone(),
                    from: s.from.checked_add(s.by)?,
                    by: s.by.checked_neg()?,
                    op: s.op,
                })
            })
            .collect::<Result<Vec<_>, CoreError>>()?;
        let content = self
            .content
            .iter()
            .rev()
            .map(|(id, by)| Ok((id.clone(), by.checked_neg()?)))
            .collect::<Result<Vec<_>, CoreError>>()?;
        Ok(StepMap::from_parts(shifts, content))
    }

    /// `by >= 0` inserts `[from, from+by)`: positions at or after `from` (After), or after
    /// `from` (Before), move by `by`. `by < 0` removes `[s, e)` = `[from+by, from)`: positions
    /// after `e` move by `by`; positions in `[s, e]` collapse to `s` and are `deleted` with
    /// `offset = pos − s`, except at the assoc edge (After: `e` is never deleted; Before: `s`
    /// is never deleted) (Decision 16). A deleted position strictly inside `(s, e)` also sets
    /// `across`. Shifts on other tracks are ignored.
    pub fn map(&self, track: &str, pos: Rational, assoc: Assoc) -> Result<MapResult, CoreError> {
        self.map_from(0, track, pos, assoc)
    }

    /// [`StepMap::map`] through the shifts from index `first` on; recover offsets keep the
    /// shift's index in the whole map.
    fn map_from(
        &self,
        first: usize,
        track: &str,
        pos: Rational,
        assoc: Assoc,
    ) -> Result<MapResult, CoreError> {
        let mut pos = pos;
        let mut deleted = None;
        let mut across = false;
        for (i, s) in self.shifts.iter().enumerate().skip(first) {
            if s.track != track {
                continue;
            }
            let (next, offset) = map_one(s, pos, assoc)?;
            pos = next;
            if let Some(offset) = offset {
                across |= offset.is_positive() && offset < s.by.checked_neg()?;
                deleted = deleted.or(Some(Recover { shift: i, offset }));
            }
        }
        Ok(MapResult {
            pos,
            deleted,
            across,
        })
    }

    /// The position `offset` into shift `r.shift`'s inserted range, in the frame after that
    /// shift, when that shift inserts exactly `size` (`by == size`). `None` otherwise.
    pub fn recover(&self, r: Recover, size: Rational) -> Result<Option<Rational>, CoreError> {
        match self.shifts.get(r.shift) {
            Some(s) if s.by == size && s.by.is_positive() => {
                Ok(Some(s.from.checked_add(r.offset)?))
            }
            _ => Ok(None),
        }
    }
}

/// The content offsets `diff`'s changes record (`(id, by)` for each change with a `by`).
pub(crate) fn content_of(diff: &Diff) -> Vec<(String, Rational)> {
    diff.changes
        .iter()
        .filter_map(|c| c.by.map(|by| (c.id.clone(), by)))
        .collect()
}

/// One shift applied to a position on its track: the new position, and the offset into the
/// removed range when the shift deleted it.
fn map_one(
    s: &Shift,
    pos: Rational,
    assoc: Assoc,
) -> Result<(Rational, Option<Rational>), CoreError> {
    let zero = Rational::zero();
    if s.by > zero {
        let moves = match assoc {
            Assoc::After => pos >= s.from,
            Assoc::Before => pos > s.from,
        };
        let pos = if moves { pos.checked_add(s.by)? } else { pos };
        return Ok((pos, None));
    }
    let (start, end) = (s.from.checked_add(s.by)?, s.from);
    if s.by == zero || pos < start {
        return Ok((pos, None));
    }
    if pos > end {
        return Ok((pos.checked_add(s.by)?, None));
    }
    let survives = match assoc {
        Assoc::After => pos == end,
        Assoc::Before => pos == start,
    };
    let offset = (!survives).then(|| pos.checked_sub(start)).transpose()?;
    Ok((start, offset))
}

/// A position that a [`Mapping`] cannot carry to its last frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Unmapped {
    /// Strictly inside a range removed by map `step`, with no mirror to recover it through.
    Deleted { step: usize },
    /// Time arithmetic overflowed.
    Overflow,
}

/// A sequence of step maps, some pairs of which mirror each other (one inverts the other).
///
/// Mirrors pair shifts in reverse order, so each side of a pair must be one op's map (or the
/// exact inverse of one): for a rebase, push one inverse `StepMap` per earlier op of ours, the
/// intervening steps, then one map per rebased op, and mirror each inverse with its rebased op.
#[derive(Debug, Clone, Default)]
pub struct Mapping {
    maps: Vec<StepMap>,
    mirror: Vec<Option<usize>>,
    /// Per map: its mirror is one of our own ops re-run at the head ([`Mapping::set_own_mirror`]).
    own: Vec<bool>,
}

impl Mapping {
    pub fn new() -> Mapping {
        Mapping::default()
    }

    /// Appends `map`; returns its index.
    pub fn push(&mut self, map: StepMap) -> usize {
        self.maps.push(map);
        self.mirror.push(None);
        self.own.push(false);
        self.maps.len() - 1
    }

    /// Marks maps `a < b` as mirrors: `b` inverts `a`, so `b`'s shifts are `a`'s in reverse
    /// order. Panics when `a >= b`, `b` is out of range, or either map already has a mirror.
    pub fn set_mirror(&mut self, a: usize, b: usize) {
        assert!(
            a < b && b < self.maps.len(),
            "mirror ({a}, {b}) needs a < b < {}",
            self.maps.len()
        );
        assert!(
            self.mirror[a].is_none() && self.mirror[b].is_none(),
            "mirror ({a}, {b}): a map is already mirrored"
        );
        self.mirror[a] = Some(b);
        self.mirror[b] = Some(a);
    }

    /// [`Mapping::set_mirror`] for the inverse of one of our own ops (`a`) and that op re-run at
    /// the head (`b`). The re-run may record shifts of other sizes, and another number of them,
    /// than the op did where it was written (a move opens its item's length at the head, and
    /// moves the link group the head has), so a position the inverse deleted is recovered
    /// leniently (4C Tasks 9–10 review, B1): through the re-run's shift on the same track at the
    /// mirrored rank among that track's shifts, at the same offset when the two shifts have one
    /// size, else at its start (a position at the start of an item we moved stays before it).
    /// A position at the surviving edge (the end, with `After`) maps as any other, so it stays
    /// past what the head inserted there.
    pub fn set_own_mirror(&mut self, a: usize, b: usize) {
        self.set_mirror(a, b);
        self.own[a] = true;
        self.own[b] = true;
    }

    pub fn len(&self) -> usize {
        self.maps.len()
    }

    pub fn is_empty(&self) -> bool {
        self.maps.is_empty()
    }

    /// ProseMirror `Mapping.map`: a deletion at step i with mirror j > i resumes at
    /// `maps[j].recover(..)` when the two shifts have equal |by| on the same track. Otherwise a
    /// position strictly inside the removed range is `Unmapped::Deleted { step: i }`, and one at
    /// its edge carries on from the range start.
    pub fn map(&self, track: &str, pos: Rational, assoc: Assoc) -> Result<Rational, Unmapped> {
        self.map_from(0, track, pos, assoc)
    }

    /// [`Mapping::map`] through `maps[start..]` only (ProseMirror `Mapping.slice(start).map`),
    /// so a growing mapping serves every op of a rebase without copies. Mirrors to maps before
    /// `start` are never followed.
    pub fn map_from(
        &self,
        start: usize,
        track: &str,
        pos: Rational,
        assoc: Assoc,
    ) -> Result<Rational, Unmapped> {
        let mut pos = pos;
        let mut i = start;
        while i < self.maps.len() {
            let r = self.maps[i].map(track, pos, assoc).map_err(overflow)?;
            if let (Some(rec), Some(j)) = (r.deleted, self.mirror[i].filter(|&j| j > i))
                && let Some(back) = self.recover(i, j, track, rec, assoc).map_err(overflow)?
            {
                pos = settle(back, j)?;
                i = j + 1;
                continue;
            }
            pos = settle(r, i)?;
            i += 1;
        }
        Ok(pos)
    }

    /// An item-relative value `v` of `item` mapped through `maps[start..]`: plus every content
    /// offset of `item` they record (Decision 16). Mirrors need no special case: a map's
    /// inverse negates its offsets, so the pair cancels. Only overflow fails.
    pub fn map_content(&self, start: usize, item: &str, v: Rational) -> Result<Rational, Unmapped> {
        self.maps
            .iter()
            .skip(start)
            .try_fold(v, |v, m| v.checked_add(m.content_of(item)?))
            .map_err(overflow)
    }

    /// A position deleted by shift `rec.shift` of map `i`, recovered through its mirror `j` and
    /// then mapped through `j`'s later shifts. `None` when the mirrored shift is on another
    /// track or has another size.
    fn recover(
        &self,
        i: usize,
        j: usize,
        track: &str,
        rec: Recover,
        assoc: Assoc,
    ) -> Result<Option<MapResult>, CoreError> {
        let (a, b) = (&self.maps[i], &self.maps[j]);
        if self.own[i] {
            return own_recover(a, b, track, rec, assoc);
        }
        let k = b.shifts.len().wrapping_sub(1 + rec.shift);
        if a.shifts.len() != b.shifts.len() || b.shifts[k].track != track {
            return Ok(None);
        }
        let size = a.shifts[rec.shift].by.checked_neg()?;
        let mirrored = Recover {
            shift: k,
            offset: rec.offset,
        };
        let Some(pos) = b.recover(mirrored, size)? else {
            return Ok(None);
        };
        b.map_from(k + 1, track, pos, assoc).map(Some)
    }
}

/// [`Mapping::set_own_mirror`]'s recovery: a position deleted by shift `rec.shift` of `a` (the
/// inverse of our op), in `b` (the op re-run) at the shift on `track` whose rank from the end
/// among `b`'s shifts there is `rec.shift`'s rank from the start among `a`'s, at `rec.offset`
/// when both have one size and at its start otherwise, then mapped through `b`'s later shifts.
/// `None` when `b` has no such shift, or it does not insert.
fn own_recover(
    a: &StepMap,
    b: &StepMap,
    track: &str,
    rec: Recover,
    assoc: Assoc,
) -> Result<Option<MapResult>, CoreError> {
    let rank = a.shifts[..rec.shift]
        .iter()
        .filter(|s| s.track == track)
        .count();
    let on: Vec<usize> = (0..b.shifts.len())
        .filter(|&k| b.shifts[k].track == track)
        .collect();
    let Some(&k) = on.len().checked_sub(1 + rank).and_then(|r| on.get(r)) else {
        return Ok(None);
    };
    let s = &b.shifts[k];
    if !s.by.is_positive() {
        return Ok(None);
    }
    let size = a.shifts[rec.shift].by.checked_neg()?;
    let offset = if s.by == size {
        rec.offset
    } else {
        Rational::zero()
    };
    let pos = s.from.checked_add(offset)?;
    b.map_from(k + 1, track, pos, assoc).map(Some)
}

fn overflow(_: CoreError) -> Unmapped {
    Unmapped::Overflow
}

/// The mapped position, unless map `step` removed a range with it strictly inside.
fn settle(r: MapResult, step: usize) -> Result<Rational, Unmapped> {
    if r.across {
        Err(Unmapped::Deleted { step })
    } else {
        Ok(r.pos)
    }
}
