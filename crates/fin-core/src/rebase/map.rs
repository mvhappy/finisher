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

/// One step's shifts, in op order; each is in the frame after the previous one. Cloning is
/// cheap (the shifts are shared), so one map serves every rebase that passes through it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct StepMap {
    shifts: Arc<[Shift]>,
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
    pub fn from_shifts(shifts: Vec<Shift>) -> StepMap {
        StepMap {
            shifts: shifts.into(),
        }
    }

    pub fn from_diff(diff: &Diff) -> StepMap {
        StepMap::from_shifts(diff.shifts.clone())
    }

    pub fn shifts(&self) -> &[Shift] {
        &self.shifts
    }

    /// The inverse map: shifts in reverse order, each `{from, by}` → `{from + by, -by}`.
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
                })
            })
            .collect::<Result<Vec<_>, CoreError>>()?;
        Ok(StepMap::from_shifts(shifts))
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
}

impl Mapping {
    pub fn new() -> Mapping {
        Mapping::default()
    }

    /// Appends `map`; returns its index.
    pub fn push(&mut self, map: StepMap) -> usize {
        self.maps.push(map);
        self.mirror.push(None);
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
