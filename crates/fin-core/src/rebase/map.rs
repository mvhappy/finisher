//! Exact per-track shift maps and mirrored mappings (ProseMirror `StepMap` / `Mapping`
//! semantics, Decision 16).
//!
//! A [`StepMap`] is one record's `Diff.shifts`, in op order, each shift in the frame left by the
//! previous one. `Shift { from, by }` with `by > 0` inserts `[from, from + by)`; with `by < 0` it
//! removes `[from + by, from)`. Its inverse is `{ from + by, -by }`.
//!
//! A position inside a removed range is collapsed to the range start and reported as deleted,
//! with its offset into the range, so a [`Mapping`] can recover it through a mirror: a later map
//! that re-inserts the same range (the rebased copy of the step whose inverse removed it).

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

/// One step's shifts, in op order; each is in the frame after the previous one.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct StepMap {
    shifts: Vec<Shift>,
}

/// Where a deleted position was: `offset` into the range removed by shift `shift`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Recover {
    pub shift: usize,
    pub offset: Rational,
}

/// A position mapped through a [`StepMap`], and the first shift that deleted it, if any.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MapResult {
    pub pos: Rational,
    pub deleted: Option<Recover>,
}

impl StepMap {
    pub fn from_shifts(shifts: Vec<Shift>) -> StepMap {
        StepMap { shifts }
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
            .collect::<Result<_, CoreError>>()?;
        Ok(StepMap { shifts })
    }

    /// `by >= 0` inserts `[from, from+by)`: positions at or after `from` (After), or after
    /// `from` (Before), move by `by`. `by < 0` removes `[s, e)` = `[from+by, from)`: positions
    /// after `e` move by `by`; positions in `[s, e]` collapse to `s` and are `deleted` with
    /// `offset = pos − s`, except at the assoc edge (After: `e` is never deleted; Before: `s`
    /// is never deleted) (Decision 16). Shifts on other tracks are ignored.
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
        for (i, s) in self.shifts.iter().enumerate().skip(first) {
            if s.track != track {
                continue;
            }
            let (next, offset) = map_one(s, pos, assoc)?;
            pos = next;
            if deleted.is_none() {
                deleted = offset.map(|offset| Recover { shift: i, offset });
            }
        }
        Ok(MapResult { pos, deleted })
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
    if s.by == zero {
        return Ok((pos, None));
    }
    let (start, end) = (s.from.checked_add(s.by)?, s.from);
    if pos < start {
        return Ok((pos, None));
    }
    if pos > end {
        return Ok((pos.checked_add(s.by)?, None));
    }
    let survives = match assoc {
        Assoc::After => pos == end,
        Assoc::Before => pos == start,
    };
    let offset = if survives {
        None
    } else {
        Some(pos.checked_sub(start)?)
    };
    Ok((start, offset))
}

/// A position that a [`Mapping`] cannot carry to its last frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Unmapped {
    /// Deleted by map `step`, with no mirror to recover it through.
    Deleted { step: usize },
    /// Time arithmetic overflowed.
    Overflow,
}

/// A sequence of step maps, some pairs of which mirror each other (one inverts the other).
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
    /// order. Panics when either index is out of range or `a >= b`.
    pub fn set_mirror(&mut self, a: usize, b: usize) {
        assert!(
            a < b && b < self.maps.len(),
            "mirror ({a}, {b}) needs a < b < {}",
            self.maps.len()
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
    /// `maps[j].recover(..)` when the two shifts have equal |by| on the same track; otherwise
    /// `Unmapped::Deleted { step: i }`.
    pub fn map(&self, track: &str, pos: Rational, assoc: Assoc) -> Result<Rational, Unmapped> {
        let overflow = |_| Unmapped::Overflow;
        let mut pos = pos;
        let mut i = 0;
        while i < self.maps.len() {
            let r = self.maps[i].map(track, pos, assoc).map_err(overflow)?;
            let Some(rec) = r.deleted else {
                pos = r.pos;
                i += 1;
                continue;
            };
            let j = self.mirror[i]
                .filter(|&j| j > i)
                .ok_or(Unmapped::Deleted { step: i })?;
            let r = self
                .recover(i, j, track, rec, assoc)
                .map_err(overflow)?
                .ok_or(Unmapped::Deleted { step: i })?;
            if r.deleted.is_some() {
                return Err(Unmapped::Deleted { step: j });
            }
            pos = r.pos;
            i = j + 1;
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
        if a.shifts.len() != b.shifts.len() {
            return Ok(None);
        }
        let k = b.shifts.len() - 1 - rec.shift;
        if b.shifts[k].track != track {
            return Ok(None);
        }
        let size = a.shifts[rec.shift].by.checked_neg()?;
        let Some(pos) = b.recover(
            Recover {
                shift: k,
                offset: rec.offset,
            },
            size,
        )?
        else {
            return Ok(None);
        };
        b.map_from(k + 1, track, pos, assoc).map(Some)
    }
}
