//! One branch of the model: clip lists, placement stamps, edge histories, and the re-run of a
//! list operation (module docs in `mod.rs`).

use std::collections::HashMap;

use super::MAX_FRAMES;
use super::lops::{EDGE, GONE, GONE_EDGE, GONE_PARAM, LOp, RESTORE, THERE, UNSURE};
use super::model::ARec;

/// A gap's entry (no id): a lift removal's, until the strict trim of the same inverse fills it.
pub const GAP: &str = "";

/// A re-run that is impossible here: the policy code, and the ids its error names.
pub type Fail = (&'static str, Vec<String>);

#[derive(Debug, Clone, Default)]
pub struct MBranch {
    /// Per track, the ordered `(clipId, frames)`; a [`GAP`] entry only inside one record.
    pub tracks: [Vec<(String, i64)>; 2],
    /// Whether a track holds a placement the model could not predict (checked by membership
    /// and durations, then the real order is taken).
    pub unsure: [bool; 2],
    /// Per clip, how often it was removed or moved on this branch.
    pub removals: HashMap<String, u32>,
    /// Per track, the no-shift ops it has seen (splits and their inverses).
    pub ns: [u32; 2],
    /// Per clip, the trims of its end in effect, oldest first (by `eff`).
    pub edges: HashMap<String, Vec<String>>,
    pub recs: Vec<ARec>,
}

impl MBranch {
    pub fn find(&self, id: &str) -> Option<(usize, usize)> {
        (0..2).find_map(|t| self.pos(t, id).map(|i| (t, i)))
    }

    fn pos(&self, t: usize, id: &str) -> Option<usize> {
        self.tracks[t].iter().position(|c| c.0 == id)
    }

    pub fn removals(&self, id: Option<&String>) -> u32 {
        id.map_or(0, |id| self.removals.get(id).copied().unwrap_or(0))
    }

    pub fn rec(&self, tx: &str) -> &ARec {
        self.recs.iter().find(|r| r.tx == tx).unwrap()
    }

    pub fn has_gap(&self, t: usize) -> bool {
        self.tracks[t].iter().any(|c| c.0 == GAP)
    }

    /// Adjacent gaps merged; empty and trailing ones dropped (nothing follows them).
    fn norm(&mut self, t: usize) {
        let mut out: Vec<(String, i64)> = Vec::new();
        for c in self.tracks[t].drain(..) {
            match out.last_mut() {
                Some(last) if last.0 == GAP && c.0 == GAP => last.1 += c.1,
                _ if c.0 == GAP && c.1 == 0 => {}
                _ => out.push(c),
            }
        }
        if out.last().is_some_and(|c| c.0 == GAP) {
            out.pop();
        }
        self.tracks[t] = out;
    }

    /// Whether a placement on `t` before `before` (or at the end) is certain: in a re-run on
    /// the same branch, while the anchor clip is there and was neither removed nor moved since
    /// (the end: while no no-shift op touched the track since, and the model knew the track's
    /// order when the op was made); in a copy, while the anchor was never removed or moved on
    /// either side and neither side's track saw a no-shift op.
    fn certain(&self, t: usize, before: Option<&String>, stamp: u32, ns: u32, copy: bool) -> bool {
        let found = before.is_none_or(|b| self.pos(t, b).is_some());
        let here = self.removals(before);
        if copy {
            found && self.ns[t] == 0 && ns == 0 && (before.is_none() || here == 0 && stamp == 0)
        } else {
            match before {
                None => stamp != UNSURE && self.ns[t] == ns,
                Some(_) => found && here == stamp,
            }
        }
    }

    /// [`Self::certain`] for a move's target; staying where it is (`before` the clip itself)
    /// is certain in a copy only on its own track, never moved or removed on either side.
    fn move_certain(&self, op: &LOp, copy: bool) -> bool {
        let LOp::Move {
            id,
            to,
            before,
            stamp,
            ns,
            ..
        } = op
        else {
            return true;
        };
        if before.as_deref() == Some(id.as_str()) {
            let on = self.find(id).is_some_and(|(t, _)| t == *to);
            !copy
                || on
                    && self.ns[*to] == 0
                    && *ns == 0
                    && self.removals(Some(id)) == 0
                    && *stamp == 0
        } else {
            self.certain(*to, before.as_ref(), *stamp, *ns, copy)
        }
    }

    /// Whether a lift restore lands where the model says: never in a copy; right after its
    /// anchor while that is there, was neither removed nor moved since and its track saw no
    /// other no-shift op; at the end while no no-shift op touched the track since; and only on
    /// a track whose order the model knows.
    fn lift_certain(&self, op: &LOp, copy: bool) -> bool {
        let LOp::RestoreLift {
            track,
            after,
            stamp,
            ns,
            ..
        } = op
        else {
            return true;
        };
        let at = match after {
            // The undo's strict trim and the redo's own came since its lift removal, and no
            // other no-shift op: a later split of the anchor would cut a part whose move or
            // removal takes the position along.
            Some(a) => {
                self.pos(*track, a).is_some()
                    && self.removals(Some(a)) == *stamp
                    && self.ns[*track] == ns.saturating_add(2)
            }
            None => *stamp != UNSURE && self.ns[*track] == *ns,
        };
        !copy && !self.unsure[*track] && at
    }

    /// Whether `op` is one whose position the model cannot follow (module docs,
    /// *Exclusions*): an uncertain placement on a track that saw a no-shift op (here, or on
    /// the source side for a copy), any uncertain lift restore, and a strict trim that grows
    /// into a track whose order the model does not know.
    pub fn risky(&self, op: &LOp, copy: bool) -> bool {
        match op {
            LOp::Insert {
                track,
                before,
                stamp,
                ns,
                ..
            } => {
                !self.certain(*track, before.as_ref(), *stamp, *ns, copy)
                    && (self.ns[*track] > 0 || *ns > 0)
            }
            LOp::Move { to, ns, .. } => {
                !self.move_certain(op, copy) && (self.ns[*to] > 0 || *ns > 0)
            }
            LOp::RestoreLift { .. } => !self.lift_certain(op, copy),
            LOp::Trim {
                id,
                strict: true,
                delta,
                ..
            } if *delta > 0 => self.find(id).is_some_and(|(t, _)| self.unsure[t]),
            _ => false,
        }
    }

    /// The independent oracle for `op` here: the reasons the engine may give when the op
    /// cannot be carried, from what the clip lists and edge histories say alone (no steps, no
    /// mirrors). A clip to remove, trim, move or split is gone; a clip to put back (or a split's
    /// right part) is there; an end edge (a trim's, a split's) someone else changed since (an
    /// undo or redo: since the edit it inverts; a copy: since what the source had before it).
    pub fn verdict(&self, op: &LOp, copy: bool) -> Option<&'static [&'static str]> {
        let present = self.find(op.clip()).is_some();
        // The trims in effect, in any order: an undo and a redo of it take one out and put it
        // back last.
        let sorted = |v: &Vec<String>| {
            let mut v = v.clone();
            v.sort();
            v
        };
        let edge = |id: &str, want: Option<&Vec<String>>| {
            let here = sorted(&self.edges.get(id).cloned().unwrap_or_default());
            want.is_some_and(|w| sorted(w) != here)
        };
        match op {
            LOp::Insert { .. } | LOp::RestoreLift { .. } if present => Some(THERE),
            // A restore puts its clip back whole, end edge included (`Footprint::overwrites`).
            LOp::Insert {
                id, expect, pre, ..
            } => edge(id, if copy { Some(pre) } else { expect.as_ref() }).then_some(RESTORE),
            LOp::RestoreLift { .. } => None,
            LOp::Remove { .. } | LOp::RemoveLift { .. } => (!present).then_some(GONE),
            LOp::Move { .. } => (!present).then_some(GONE_PARAM),
            _ if !present => Some(GONE_EDGE),
            LOp::Trim {
                id, expect, pre, ..
            } => edge(id, if copy { Some(pre) } else { expect.as_ref() }).then_some(EDGE),
            LOp::Split { id, right, pre, .. } => {
                if edge(id, copy.then_some(pre)) {
                    Some(EDGE)
                } else {
                    self.find(right).is_some().then_some(THERE)
                }
            }
        }
    }

    /// Applies `op` (the re-run: `Err` when it is impossible here); returns it as made.
    /// `copy`: it comes from another branch (placement certainty, [`Self::certain`]).
    pub fn run(&mut self, op: &LOp, copy: bool) -> Result<LOp, Fail> {
        let risky = self.risky(op, copy);
        let gone = |id: &String| ("NOT_FOUND", vec![id.clone()]);
        match op.clone() {
            LOp::Insert {
                track,
                id,
                dur,
                before,
                stamp,
                ns,
                expect,
                ..
            } => {
                if self.find(&id).is_some() {
                    return Err(("DUPLICATE_ID", vec![id]));
                }
                let sure = self.certain(track, before.as_ref(), stamp, ns, copy);
                let i = match &before {
                    Some(b) if sure => self.pos(track, b).unwrap(),
                    _ => self.tracks[track].len(),
                };
                self.tracks[track].insert(i, (id.clone(), dur));
                self.unsure[track] |= !sure;
                let stamp = if sure && !self.unsure[track] {
                    self.removals(before.as_ref())
                } else {
                    UNSURE
                };
                let pre = self.edges.get(&id).cloned().unwrap_or_default();
                Ok(LOp::Insert {
                    track,
                    id,
                    dur,
                    before,
                    stamp,
                    ns: self.ns[track],
                    expect,
                    pre,
                })
            }
            LOp::Remove { id, .. } => {
                let (t, i) = self.find(&id).ok_or_else(|| gone(&id))?;
                let (_, dur) = self.tracks[t].remove(i);
                let (succ, stamp, ns) = self.after_gap(t, i);
                *self.removals.entry(id.clone()).or_default() += 1;
                self.norm(t);
                let post = self.edges.get(&id).cloned().unwrap_or_default();
                Ok(LOp::Remove {
                    track: t,
                    id,
                    dur,
                    succ,
                    stamp,
                    ns,
                    post,
                })
            }
            LOp::Trim {
                id,
                delta,
                strict,
                eff,
                push,
                expect,
                ..
            } => {
                let (t, i) = self.find(&id).ok_or_else(|| gone(&id))?;
                let d = self.tracks[t][i].1 + delta;
                if !(1..=MAX_FRAMES).contains(&d) {
                    return Err(("INVALID_RANGE", vec![id]));
                }
                if strict {
                    self.strict(t, i, delta, risky)?;
                }
                self.tracks[t][i].1 = d;
                self.norm(t);
                let hist = self.edges.entry(id.clone()).or_default();
                let pre = hist.clone();
                if push {
                    hist.push(eff.clone());
                } else if let Some(k) = hist.iter().rposition(|e| *e == eff) {
                    hist.remove(k);
                }
                Ok(LOp::Trim {
                    id,
                    delta,
                    strict,
                    eff,
                    push,
                    expect,
                    pre,
                    post: hist.clone(),
                })
            }
            LOp::Move { id, to, before, .. } => {
                let (from, i) = self.find(&id).ok_or_else(|| gone(&id))?;
                let sure = self.move_certain(op, copy);
                let stay = before.as_deref() == Some(id.as_str());
                let was = self.removals(Some(&id));
                let (_, dur) = self.tracks[from].remove(i);
                let (succ, sstamp, sns) = self.after_gap(from, i);
                *self.removals.entry(id.clone()).or_default() += 1;
                let j = match &before {
                    _ if stay && sure => i,
                    Some(b) if sure => self.pos(to, b).unwrap(),
                    _ => self.tracks[to].len(),
                };
                self.tracks[to].insert(j, (id.clone(), dur));
                self.unsure[to] |= !sure;
                self.norm(from);
                self.norm(to);
                let stamp = if stay {
                    was
                } else if sure && !self.unsure[to] {
                    self.removals(before.as_ref())
                } else {
                    UNSURE
                };
                let post = self.edges.get(&id).cloned().unwrap_or_default();
                Ok(LOp::Move {
                    id,
                    to,
                    before,
                    stamp,
                    ns: self.ns[to],
                    from,
                    dur,
                    succ,
                    sstamp,
                    sns,
                    post,
                })
            }
            LOp::Split {
                id,
                right,
                off,
                eff,
                ..
            } => {
                let (t, i) = self.find(&id).ok_or_else(|| gone(&id))?;
                let dur = self.tracks[t][i].1;
                if !(1..dur).contains(&off) {
                    return Err(("INVALID_RANGE", vec![id]));
                }
                if self.find(&right).is_some() {
                    return Err(("DUPLICATE_ID", vec![right]));
                }
                self.tracks[t][i].1 = off;
                self.tracks[t].insert(i + 1, (right.clone(), dur - off));
                self.ns[t] += 1;
                let hist = self.edges.entry(id.clone()).or_default();
                let pre = hist.clone();
                hist.push(eff.clone());
                Ok(LOp::Split {
                    id,
                    right,
                    off,
                    rlen: dur - off,
                    eff,
                    pre,
                    post: hist.clone(),
                })
            }
            LOp::RemoveLift { id, partner, .. } => {
                let (t, i) = self.find(&id).ok_or_else(|| gone(&id))?;
                let (_, dur) = self.tracks[t].remove(i);
                let after = i
                    .checked_sub(1)
                    .map(|j| self.tracks[t][j].0.clone())
                    .filter(|p| *p == partner);
                if i < self.tracks[t].len() {
                    self.tracks[t].insert(i, (GAP.to_string(), dur));
                }
                let stamp = if self.unsure[t] {
                    UNSURE
                } else {
                    self.removals(after.as_ref())
                };
                *self.removals.entry(id.clone()).or_default() += 1;
                self.ns[t] += 1;
                self.norm(t);
                Ok(LOp::RemoveLift {
                    track: t,
                    id,
                    dur,
                    partner,
                    after,
                    stamp,
                    ns: self.ns[t],
                })
            }
            LOp::RestoreLift {
                track,
                id,
                dur,
                partner,
                after,
                ..
            } => {
                if self.find(&id).is_some() {
                    return Err(("DUPLICATE_ID", vec![id]));
                }
                let t = track;
                let sure = !risky;
                let i = match after.as_ref().and_then(|a| self.pos(t, a)) {
                    Some(p) => p + 1,
                    None => self.tracks[t].len(),
                };
                match self.tracks[t].get(i).cloned() {
                    Some((g, len)) if g == GAP && len >= dur => self.tracks[t][i].1 -= dur,
                    Some((next, _)) if sure => {
                        let n = if next == GAP {
                            self.tracks[t][i + 1].0.clone()
                        } else {
                            next
                        };
                        return Err(("CLIP_OVERLAP", vec![id, n]));
                    }
                    _ => {}
                }
                self.tracks[t].insert(i, (id.clone(), dur));
                self.unsure[t] |= !sure;
                self.ns[t] += 1;
                self.norm(t);
                let stamp = if sure && !self.unsure[t] {
                    self.removals(after.as_ref())
                } else {
                    UNSURE
                };
                Ok(LOp::RestoreLift {
                    track: t,
                    id,
                    dur,
                    partner,
                    after,
                    stamp,
                    ns: self.ns[t],
                })
            }
        }
    }

    /// What follows a clip just taken from index `i` of `t`, for its inverse's placement: the
    /// clip there, its stamp and the track's no-shift count (both [`UNSURE`] when a gap is
    /// there or the order unknown).
    fn after_gap(&self, t: usize, i: usize) -> (Option<String>, u32, u32) {
        let next = self.tracks[t].get(i);
        let gap = next.is_some_and(|c| c.0 == GAP);
        let succ = next.filter(|c| c.0 != GAP).map(|c| c.0.clone());
        if gap || self.unsure[t] {
            let ns = if gap { UNSURE } else { self.ns[t] };
            return (succ, UNSURE, ns);
        }
        let stamp = self.removals(succ.as_ref());
        (succ, stamp, self.ns[t])
    }

    /// A strict trim of clip `i` on `t` by `delta`: it grows into the gap after it (an
    /// overlap is `CLIP_OVERLAP`, unless `risky`: the model does not know what follows) or
    /// shrinks, leaving a gap when something follows; the track saw a no-shift op.
    fn strict(&mut self, t: usize, i: usize, delta: i64, risky: bool) -> Result<(), Fail> {
        let id = self.tracks[t][i].0.clone();
        if delta > 0 {
            match self.tracks[t].get(i + 1).cloned() {
                None => {}
                Some((g, len)) if g == GAP && len >= delta => self.tracks[t][i + 1].1 -= delta,
                Some((g, _)) if g == GAP && risky => {
                    self.tracks[t].remove(i + 1);
                }
                Some((g, _)) if g == GAP => {
                    let n = self.tracks[t][i + 2].0.clone();
                    return Err(("CLIP_OVERLAP", vec![id, n]));
                }
                Some(_) if risky => {}
                Some((n, _)) => return Err(("CLIP_OVERLAP", vec![id, n])),
            }
        } else if i + 1 < self.tracks[t].len() {
            self.tracks[t].insert(i + 1, (GAP.to_string(), -delta));
        }
        self.ns[t] += 1;
        Ok(())
    }
}
