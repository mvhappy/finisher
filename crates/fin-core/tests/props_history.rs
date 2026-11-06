//! Undo/redo property (spec §4.6 prop 4; plan 4B Task 20): 2–4 authors interleave edits,
//! undos, redos, cherry-picks and merges on `main` and a side branch `br1`, through the real
//! planners over a [`MemLog`]; after every step the heads, the stacks and every conflict
//! reason match a reference model.
//!
//! **Edits** are gapless ripple edits: inserts at a clip boundary or at the track end, deletes
//! and end trims, one op each. A track is then exactly its ordered list of `(clipId,
//! duration)`.
//!
//! **The model** knows nothing of positions. It keeps:
//! - per branch, the ordered clip lists, changed by list operations: insert an id before its
//!   recorded successor id (or at the end), remove an id, add to a duration;
//! - per author and branch, undo and redo stacks of unit ids (Decisions 22–23);
//! - per record, the list operations it made, so a unit's inverse is a list of list
//!   operations (a removal records the successor it had, and its inverse reinserts before it).
//!
//! **Placement.** A reinsertion is certain while its successor is present and was not removed
//! since (a copy: never removed on either side). Otherwise the model checks membership and
//! durations only, then takes the real order (the ProseMirror tie-break: Bob deletes c3, Alice
//! deletes c2, Bob undoes, Alice undoes gives `c1, c3, c2`, which the recorded successors
//! predict exactly).
//!
//! **Conflicts** are predicted from abstract footprints (an insert or restore creates its clip,
//! a removal needs and removes it, a trim needs its end edge) over the records between a
//! unit and the head, or over a copy's merge steps (the source records before it inverted,
//! the target's records, the copies so far), paired as Decision 24 says: an undo with its
//! units, a redo with its undo, a copy with its inverted original, a target record with the
//! inverted source record of the same origin root. Then:
//! - undo of an insert is `REMOVED` when another author deleted the clip;
//! - undo of a trim is `SAME_EDGE` when someone later trimmed the same end and that still
//!   stands, `REMOVED` when the clip is gone;
//! - a copy (merge or cherry-pick) is the same list operation on the target; it fails when
//!   that is impossible (`REMOVED`, `SAME_EDGE`, `ALREADY_PRESENT`), as `DEPENDS_ON_SKIPPED`
//!   when what makes it impossible is a source record the target does not hold;
//! - a copy of an undo or redo whose unit the merge skipped (a pick: the target does not hold)
//!   is `DEPENDS_ON_SKIPPED`: its inverse would bring back what the target never lost (found
//!   by this property: a skipped delete's undo re-created a clip the target had deleted).

mod common;

use std::collections::{BTreeMap, HashMap, HashSet};

use common::memlog::MemLog;
use common::*;
use fin_core::history::UnitEntry;
use fin_core::rebase::{Conflict, reasons};
use fin_core::{CoreError, Edge, Op, Project, Rational, apply_tx};
use proptest::prelude::*;
use proptest::sample::Index;

const AUTHORS: [&str; 4] = ["alice", "bob", "carol", "dave"];
const BR: [&str; 2] = ["main", "br1"];
const TRACKS: [&str; 2] = ["t1", "t2"];
const MAIN: usize = 0;
const SIDE: usize = 1;
/// Asset `a1` is 100 s long and every clip starts 1 s into it.
const MAX_FRAMES: i64 = 99 * 30;

fn frames(n: i64) -> Rational {
    Rational::new(n, 30).unwrap()
}

fn to_frames(r: Rational) -> i64 {
    assert_eq!((r.num() * 30) % r.den(), 0, "{r} is not a whole frame");
    r.num() * 30 / r.den()
}

/// `a1`, and clips of 40, 50, 30 frames on `t1` and 60, 45 on `t2`.
fn genesis() -> Project {
    let mut ops = vec![av_asset("$a", secs(100))];
    for (t, lens) in [[40, 50, 30].as_slice(), &[60, 45]].iter().enumerate() {
        let mut at = 0;
        for (i, n) in lens.iter().enumerate() {
            let id = format!("$c{t}{i}");
            ops.push(insert(
                &id,
                TRACKS[t],
                frames(at),
                "$a",
                secs(1),
                frames(*n),
            ));
            at += n;
        }
    }
    apply(&project(), ops).project
}

// ---------------------------------------------------------------------------------------------
// The driver.

#[derive(Debug, Clone)]
enum Act {
    Insert {
        side: bool,
        track: usize,
        at: usize,
        len: i64,
    },
    Delete {
        side: bool,
        track: usize,
        k: usize,
    },
    Trim {
        side: bool,
        track: usize,
        k: usize,
        frames: i64,
    },
    Undo {
        side: bool,
    },
    Redo {
        side: bool,
    },
    /// A cherry-pick of the side branch's record `k` (mod its count) into main.
    Pick {
        k: usize,
    },
    /// A merge of the side branch into main.
    Merge {
        skip: bool,
    },
}

fn act() -> impl Strategy<Value = Act> {
    prop_oneof![
        3 => (any::<bool>(), 0usize..2, 0usize..5, 5i64..40)
            .prop_map(|(side, track, at, len)| Act::Insert { side, track, at, len }),
        2 => (any::<bool>(), 0usize..2, 0usize..5)
            .prop_map(|(side, track, k)| Act::Delete { side, track, k }),
        3 => (any::<bool>(), 0usize..2, 0usize..5, prop_oneof![-9i64..=-1, 1i64..=9])
            .prop_map(|(side, track, k, frames)| Act::Trim { side, track, k, frames }),
        3 => any::<bool>().prop_map(|side| Act::Undo { side }),
        2 => any::<bool>().prop_map(|side| Act::Redo { side }),
        1 => (0usize..16).prop_map(|k| Act::Pick { k }),
        1 => any::<bool>().prop_map(|skip| Act::Merge { skip }),
    ]
}

/// 2–4 authors with 0–12 acts each, interleaved: `(author, act)` in the order they run.
fn script() -> impl Strategy<Value = Vec<(usize, Act)>> {
    (2usize..=4)
        .prop_flat_map(|n| {
            (
                prop::collection::vec(prop::collection::vec(act(), 0..=12), n),
                prop::collection::vec(any::<Index>(), 48),
            )
        })
        .prop_map(|(mut per, picks)| {
            for acts in &mut per {
                acts.reverse();
            }
            let mut out = Vec::new();
            for pick in picks.iter().cycle() {
                let left: Vec<usize> = (0..per.len()).filter(|&a| !per[a].is_empty()).collect();
                if left.is_empty() {
                    break;
                }
                let a = left[pick.index(left.len())];
                out.push((a, per[a].pop().unwrap()));
            }
            out
        })
}

// ---------------------------------------------------------------------------------------------
// The model: list operations.

/// A list operation on one branch. `stamp`: how often the clip it is placed before had been
/// removed on its branch when the operation was planned (an insert) or made (a removal's
/// successor); `UNSURE` when the model did not know the successor.
#[derive(Debug, Clone, PartialEq)]
enum LOp {
    Insert {
        track: usize,
        id: String,
        dur: i64,
        before: Option<String>,
        stamp: u32,
    },
    Remove {
        track: usize,
        id: String,
        dur: i64,
        succ: Option<String>,
        stamp: u32,
    },
    Trim {
        id: String,
        delta: i64,
    },
}

const UNSURE: u32 = u32::MAX;

impl LOp {
    /// The inverse, as planned (a removal's successor is found when it is made).
    fn inverse(&self) -> LOp {
        match self.clone() {
            LOp::Insert { track, id, dur, .. } => LOp::Remove {
                track,
                id,
                dur,
                succ: None,
                stamp: 0,
            },
            LOp::Remove {
                track,
                id,
                dur,
                succ,
                stamp,
            } => LOp::Insert {
                track,
                id,
                dur,
                before: succ,
                stamp,
            },
            LOp::Trim { id, delta } => LOp::Trim { id, delta: -delta },
        }
    }

    fn fp(&self) -> Fp {
        let mut f = Fp::default();
        match self {
            LOp::Insert { id, .. } => f.creates.push(id.clone()),
            LOp::Remove { id, .. } => {
                f.touches.push((id.clone(), false));
                f.removes.push(id.clone());
            }
            LOp::Trim { id, .. } => f.touches.push((id.clone(), true)),
        }
        f
    }

    fn clip(&self) -> &str {
        match self {
            LOp::Insert { id, .. } | LOp::Remove { id, .. } | LOp::Trim { id, .. } => id,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
enum Kind {
    Edit,
    Undo(String),
    Redo(String),
    Merge(String),
    Pick,
}

/// A record as the model sees it: its kind, origin root and list operations.
#[derive(Debug, Clone)]
struct ARec {
    tx: String,
    seq: u64,
    kind: Kind,
    root: String,
    ops: Vec<LOp>,
}

#[derive(Debug, Clone)]
struct Unit {
    id: String,
    records: Vec<String>,
    last_seq: u64,
    /// The redo record a redo unit takes its inverse from.
    redo: Option<String>,
}

#[derive(Debug, Clone, Default)]
struct Stacks {
    undo: Vec<Unit>,
    redo: Vec<Unit>,
}

#[derive(Debug, Clone, Default)]
struct MBranch {
    tracks: [Vec<(String, i64)>; 2],
    /// Whether a track holds a placement the model could not predict (checked by membership
    /// and durations, then the real order is taken).
    unsure: [bool; 2],
    removals: HashMap<String, u32>,
    recs: Vec<ARec>,
}

impl MBranch {
    fn find(&self, id: &str) -> Option<(usize, usize)> {
        (0..2).find_map(|t| {
            self.tracks[t]
                .iter()
                .position(|c| c.0 == id)
                .map(|i| (t, i))
        })
    }

    fn removals(&self, id: Option<&String>) -> u32 {
        id.map_or(0, |id| self.removals.get(id).copied().unwrap_or(0))
    }

    fn rec(&self, tx: &str) -> &ARec {
        self.recs.iter().find(|r| r.tx == tx).unwrap()
    }

    /// Applies `op` (the re-run: `Err(code)` when it is impossible here); returns it as made.
    /// `copy`: it comes from another branch, so a placement is certain only before a clip
    /// never removed on either side.
    fn run(&mut self, op: &LOp, copy: bool) -> Result<LOp, &'static str> {
        match op.clone() {
            LOp::Insert {
                track,
                id,
                dur,
                before,
                stamp,
            } => {
                if self.find(&id).is_some() {
                    return Err("DUPLICATE_ID");
                }
                let at = before.as_ref().and_then(|b| {
                    let i = self.tracks[track].iter().position(|c| c.0 == *b)?;
                    let here = self.removals(Some(b));
                    let sure = if copy {
                        here == 0 && stamp == 0
                    } else {
                        here == stamp
                    };
                    sure.then_some(i)
                });
                let sure = before.is_none() || at.is_some();
                let i = at.unwrap_or(self.tracks[track].len());
                self.tracks[track].insert(i, (id.clone(), dur));
                self.unsure[track] |= !sure;
                let stamp = if sure && !self.unsure[track] {
                    self.removals(before.as_ref())
                } else {
                    UNSURE
                };
                Ok(LOp::Insert {
                    track,
                    id,
                    dur,
                    before,
                    stamp,
                })
            }
            LOp::Remove { id, .. } => {
                let (t, i) = self.find(&id).ok_or("NOT_FOUND")?;
                let (_, dur) = self.tracks[t].remove(i);
                let succ = self.tracks[t].get(i).map(|c| c.0.clone());
                let stamp = if self.unsure[t] {
                    UNSURE
                } else {
                    self.removals(succ.as_ref())
                };
                *self.removals.entry(id.clone()).or_default() += 1;
                Ok(LOp::Remove {
                    track: t,
                    id,
                    dur,
                    succ,
                    stamp,
                })
            }
            LOp::Trim { id, delta } => {
                let (t, i) = self.find(&id).ok_or("NOT_FOUND")?;
                let d = self.tracks[t][i].1 + delta;
                if !(1..=MAX_FRAMES).contains(&d) {
                    return Err("INVALID_RANGE");
                }
                self.tracks[t][i].1 = d;
                Ok(op.clone())
            }
        }
    }
}

// ---------------------------------------------------------------------------------------------
// The model: abstract steps, mirrors and the entity rules (Decision 24, spec §4.3).

/// An op's abstract footprint: `touches` (id, is the end edge), creates, removes.
#[derive(Debug, Clone, Default)]
struct Fp {
    touches: Vec<(String, bool)>,
    creates: Vec<String>,
    removes: Vec<String>,
}

#[derive(Debug, Clone)]
struct AStep {
    tx: String,
    fps: Vec<Fp>,
    mirror_of: Option<usize>,
    /// The record whose edit this step applies again (`false`) or takes back (`true`).
    unit: Option<(String, bool)>,
}

impl AStep {
    fn forward(r: &ARec) -> AStep {
        AStep {
            tx: r.tx.clone(),
            fps: r.ops.iter().map(LOp::fp).collect(),
            mirror_of: None,
            unit: Some((r.tx.clone(), false)),
        }
    }

    fn inverted(r: &ARec) -> AStep {
        let mut fps: Vec<Fp> = r.ops.iter().map(LOp::fp).collect();
        fps.reverse();
        for f in &mut fps {
            std::mem::swap(&mut f.creates, &mut f.removes);
        }
        AStep {
            tx: r.tx.clone(),
            fps,
            mirror_of: None,
            unit: None,
        }
    }
}

/// `recs` as steps with their mirrors: an undo is cut into one part per op of the units it
/// undoes (each unit a record, or a merge group by its id), a redo into the parts of its undo,
/// each part mirroring a step not mirrored yet.
fn steps_from(recs: &[&ARec]) -> Vec<AStep> {
    let mut steps: Vec<AStep> = Vec::new();
    let mut paired: Vec<bool> = Vec::new();
    let mut by_tx: HashMap<String, Vec<usize>> = HashMap::new();
    let mut groups: HashMap<String, Vec<usize>> = HashMap::new();
    for r in recs {
        let unit = |id: &str| -> Option<Vec<usize>> {
            by_tx
                .get(id)
                .or_else(|| groups.get(id))
                .map(|g| g.iter().rev().copied().collect())
        };
        let addressed = match &r.kind {
            Kind::Undo(u) | Kind::Redo(u) => unit(u),
            _ => None,
        }
        .filter(|t| t.iter().map(|&j| steps[j].fps.len()).sum::<usize>() == r.ops.len());
        let start = steps.len();
        match addressed {
            Some(t) => {
                let mut fps = r.ops.iter().map(LOp::fp);
                for j in t {
                    let part: Vec<Fp> = fps.by_ref().take(steps[j].fps.len()).collect();
                    let mirror = (!paired[j]).then_some(j);
                    if mirror.is_some() {
                        paired[j] = true;
                    }
                    let unit = steps[j].unit.clone().map(|(b, inv)| (b, !inv));
                    paired.push(mirror.is_some());
                    steps.push(AStep {
                        tx: r.tx.clone(),
                        fps: part,
                        mirror_of: mirror,
                        unit,
                    });
                }
            }
            None => {
                let mut s = AStep::forward(r);
                if matches!(r.kind, Kind::Undo(_) | Kind::Redo(_)) {
                    s.unit = None;
                }
                steps.push(s);
                paired.push(false);
            }
        }
        by_tx
            .entry(r.tx.clone())
            .or_default()
            .extend(start..steps.len());
        if let Kind::Merge(m) = &r.kind {
            groups
                .entry(m.clone())
                .or_default()
                .extend(start..steps.len());
        }
    }
    steps
}

fn paired(steps: &[AStep]) -> Vec<bool> {
    let mut out = vec![false; steps.len()];
    for (s, step) in steps.iter().enumerate() {
        if let Some(j) = step.mirror_of {
            out[j] = true;
            out[s] = true;
        }
    }
    out
}

/// The footprints of the steps in no mirrored pair, with their txIds.
fn unmirrored(steps: &[AStep]) -> Vec<(String, Fp)> {
    steps
        .iter()
        .zip(paired(steps))
        .filter(|(_, p)| !p)
        .flat_map(|(s, _)| s.fps.iter().map(|f| (s.tx.clone(), f.clone())))
        .collect()
}

/// The entity rules: `REMOVED` (the last unmirrored record to remove or create a needed clip
/// removed it; the earliest such record) before `SAME_EDGE` (the first unmirrored record to
/// trim the same end).
fn entity_conflict(
    ours: &Fp,
    own: &HashSet<String>,
    theirs: &[(String, Fp)],
) -> Option<(&'static str, String)> {
    let mut needed: Vec<(String, Option<usize>)> = ours
        .touches
        .iter()
        .filter(|(x, _)| !own.contains(x))
        .map(|(x, _)| (x.clone(), None))
        .collect();
    needed.dedup();
    let mut same = None;
    for (i, (_, f)) in theirs.iter().enumerate() {
        for (x, by) in needed.iter_mut() {
            if f.creates.contains(x) {
                *by = None;
            }
            if f.removes.contains(x) {
                *by = Some(i);
            }
        }
        if same.is_none()
            && ours.touches.iter().any(|(x, edge)| {
                *edge && !own.contains(x) && f.touches.contains(&(x.clone(), true))
            })
        {
            same = Some(i);
        }
    }
    let removed = needed.iter().filter_map(|(_, by)| *by).min();
    removed
        .map(|i| (reasons::REMOVED, i))
        .or(same.map(|i| (reasons::SAME_EDGE, i)))
        .map(|(r, i)| (r, theirs[i].0.clone()))
}

/// A predicted conflict: `(txId, opIndex, reason, againstTx)`.
type Pred = (String, usize, String, Option<String>);

/// `ops` re-run one by one on `b` over `steps` (module docs); the ops as made, or the conflict.
/// `dup`: the reason a re-created clip that is there already gets (a copy's same origin:
/// `ALREADY_PRESENT`; an undo's: `ID_COLLISION`).
fn rerun(
    b: &mut MBranch,
    tx: &str,
    ops: &[LOp],
    steps: &[AStep],
    copy: bool,
    dup: &'static str,
) -> Result<Vec<LOp>, Pred> {
    let theirs = unmirrored(steps);
    let mut created = HashSet::new();
    let mut made = Vec::new();
    for (k, op) in ops.iter().enumerate() {
        let f = op.fp();
        if let Some((reason, against)) = entity_conflict(&f, &created, &theirs) {
            return Err((tx.to_string(), k, reason.to_string(), Some(against)));
        }
        created.extend(f.creates.iter().cloned());
        match b.run(op, copy) {
            Ok(op) => made.push(op),
            Err(code) => {
                let x = op.clip();
                let blamed: HashSet<&str> = theirs
                    .iter()
                    .filter(|(_, f)| {
                        f.touches.iter().any(|(y, _)| y == x)
                            || f.creates.iter().any(|y| y == x)
                            || f.removes.iter().any(|y| y == x)
                    })
                    .map(|(t, _)| t.as_str())
                    .collect();
                let against =
                    (blamed.len() == 1).then(|| blamed.iter().next().unwrap().to_string());
                let reason = if code == "DUPLICATE_ID" { dup } else { code };
                return Err((tx.to_string(), k, reason.to_string(), against));
            }
        }
    }
    Ok(made)
}

/// As the merge's `on_skipped`: a conflict blamed on a record in `skipped` depends on it.
fn on_skipped(mut c: Pred, skipped: &HashSet<String>) -> Pred {
    if c.3.as_ref().is_some_and(|t| skipped.contains(t)) {
        c.2 = reasons::DEPENDS_ON_SKIPPED.to_string();
    }
    c
}

/// A merge's steps from the side branch to main (both paths are whole: the side branch forks
/// at main's seq 0): the side records inverted, main's records with their mirrors, and which
/// main step pairs with which side record by origin root.
struct Paths {
    inv: Vec<AStep>,
    target: Vec<AStep>,
    cross: Vec<(usize, usize)>,
}

impl Paths {
    fn new(source: &[ARec], target: &[ARec]) -> Paths {
        let inv = source.iter().map(AStep::inverted).collect();
        let t = steps_from(&target.iter().collect::<Vec<_>>());
        let root_of: HashMap<&str, &str> = target
            .iter()
            .map(|r| (r.tx.as_str(), r.root.as_str()))
            .collect();
        let p = paired(&t);
        let mut live: HashMap<&str, usize> = HashMap::new();
        for (q, s) in t.iter().enumerate() {
            if let Some((base, false)) = &s.unit
                && !p[q]
                && let Some(root) = root_of.get(base.as_str())
            {
                live.insert(root, q);
            }
        }
        let mut used = HashSet::new();
        let cross = source
            .iter()
            .enumerate()
            .filter_map(|(j, r)| {
                let q = *live.get(r.root.as_str())?;
                used.insert(q).then_some((q, j))
            })
            .collect();
        Paths {
            inv,
            target: t,
            cross,
        }
    }

    fn steps(&self, i: usize, planned: &[(usize, AStep)]) -> Vec<AStep> {
        let at = |j: usize| i - 1 - j;
        let mut steps: Vec<AStep> = (0..i).rev().map(|j| self.inv[j].clone()).collect();
        let mut p = vec![false; i + self.target.len() + planned.len()];
        for t in &self.target {
            let mut t = t.clone();
            t.mirror_of = t.mirror_of.map(|m| m + i);
            if let Some(m) = t.mirror_of {
                p[m] = true;
                p[steps.len()] = true;
            }
            steps.push(t);
        }
        for &(q, j) in &self.cross {
            let b = i + q;
            if j < i && !p[at(j)] && !p[b] {
                steps[b].mirror_of = Some(at(j));
                p[at(j)] = true;
                p[b] = true;
            }
        }
        for (j, step) in planned {
            let mut s = step.clone();
            if *j < i && !p[at(*j)] {
                s.mirror_of = Some(at(*j));
                p[at(*j)] = true;
                p[steps.len()] = true;
            }
            steps.push(s);
        }
        steps
    }
}

#[derive(Debug, Clone, Default)]
struct Model {
    br: [MBranch; 2],
    stacks: BTreeMap<(usize, usize), Stacks>,
    /// Side records copied into main by a merge (Decision 22: they drop the side's stacks).
    merged_away: HashSet<String>,
}

impl Model {
    fn new(p: &Project) -> Model {
        let mut b = MBranch::default();
        for t in 0..2 {
            b.tracks[t] = p.timeline.tracks[t]
                .items
                .iter()
                .map(|c| (c.id.clone(), to_frames(c.duration)))
                .collect();
        }
        Model {
            br: [b.clone(), b],
            ..Model::default()
        }
    }

    /// The stacks as derived: each drops from its top-most entry with a merged-away record.
    fn stacks(&self, b: usize, a: usize) -> Stacks {
        let mut s = self.stacks.get(&(b, a)).cloned().unwrap_or_default();
        if b == SIDE {
            for stack in [&mut s.undo, &mut s.redo] {
                if let Some(i) = stack
                    .iter()
                    .rposition(|u| u.records.iter().any(|t| self.merged_away.contains(t)))
                {
                    stack.drain(..=i);
                }
            }
        }
        s
    }

    fn raw(&mut self, b: usize, a: usize) -> &mut Stacks {
        self.stacks.entry((b, a)).or_default()
    }

    fn push(&mut self, b: usize, rec: ARec) {
        self.br[b].recs.push(rec);
    }

    /// A new unit of `a` on `b` (an edit, a pick, a merge group).
    fn push_unit(&mut self, b: usize, a: usize, id: &str, records: Vec<String>, seq: u64) {
        let s = self.raw(b, a);
        s.undo.push(Unit {
            id: id.to_string(),
            records,
            last_seq: seq,
            redo: None,
        });
        s.redo.clear();
    }

    /// The inverse ops of `unit` on `b`: its records' (or its redo record's) list operations
    /// inverted, newest first.
    fn inverse_of(&self, b: usize, unit: &Unit) -> Vec<LOp> {
        let sources = match &unit.redo {
            Some(r) => vec![r.clone()],
            None => unit.records.clone(),
        };
        sources
            .iter()
            .rev()
            .flat_map(|t| self.br[b].rec(t).ops.iter().rev().map(LOp::inverse))
            .collect()
    }

    fn after(&self, b: usize, seq: u64) -> Vec<&ARec> {
        self.br[b].recs.iter().filter(|r| r.seq > seq).collect()
    }

    /// Plans `ops` on `b` over its records after `at` (an undo or a redo).
    fn plan_inverse(&self, b: usize, tx: &str, at: u64, ops: &[LOp]) -> Result<MBranch, Pred> {
        let steps = steps_from(&self.after(b, at));
        let mut next = self.br[b].clone();
        let made = rerun(&mut next, tx, ops, &steps, false, reasons::ID_COLLISION)?;
        next.recs.push(ARec {
            tx: tx.to_string(),
            seq: 0,
            kind: Kind::Edit,
            root: tx.to_string(),
            ops: made,
        });
        Ok(next)
    }
}

// ---------------------------------------------------------------------------------------------
// One run: the real log and the model, step by step.

struct Run {
    log: MemLog,
    m: Model,
    n: usize,
}

/// The reasons of a `CONFLICT`, as predictions are written.
fn preds(e: &CoreError) -> Vec<Pred> {
    assert_eq!(e.code, "CONFLICT", "{e:?}");
    let all: Vec<Conflict> = serde_json::from_value(e.details["conflicts"].clone()).unwrap();
    all.into_iter().map(pred).collect()
}

fn pred(c: Conflict) -> Pred {
    (c.tx_id, c.op_index, c.reason, c.against_tx)
}

fn ids(stack: &[UnitEntry]) -> Vec<String> {
    stack.iter().map(|u| u.id.clone()).collect()
}

impl Run {
    fn tx(&mut self) -> String {
        self.n += 1;
        format!("x{}", self.n)
    }

    fn step(&mut self, a: usize, act: &Act) -> Result<(), TestCaseError> {
        match *act {
            Act::Insert { side, .. } | Act::Delete { side, .. } | Act::Trim { side, .. } => {
                self.edit(a, usize::from(side), act);
                Ok(())
            }
            Act::Undo { side } => self.undo(a, usize::from(side)),
            Act::Redo { side } => self.redo(a, usize::from(side)),
            Act::Pick { k } => self.pick(a, k),
            Act::Merge { skip } => self.merge(a, skip),
        }
    }

    /// An edit at the head: the op from the real state (which the model's order matches),
    /// skipped when it does not apply there (a trim past the limits, an empty track).
    fn edit(&mut self, a: usize, b: usize, act: &Act) {
        let p = self.log.state(BR[b]);
        let (op, mut lop) = match *act {
            Act::Insert { track, at, len, .. } => {
                let items = &p.timeline.tracks[track].items;
                let (pos, before) = match items.get(at) {
                    Some(c) => (c.start, Some(c.id.clone())),
                    None => (
                        items
                            .last()
                            .map_or(Rational::zero(), |c| (c.start + c.duration).unwrap()),
                        None,
                    ),
                };
                let stamp = self.m.br[b].removals(before.as_ref());
                let op = insert("$x", TRACKS[track], pos, "a1", secs(1), frames(len));
                let lop = LOp::Insert {
                    track,
                    id: String::new(),
                    dur: len,
                    before,
                    stamp,
                };
                (op, lop)
            }
            Act::Delete { track, k, .. } => {
                let items = &p.timeline.tracks[track].items;
                let Some(c) = items.get(k % items.len().max(1)) else {
                    return;
                };
                let lop = LOp::Remove {
                    track,
                    id: c.id.clone(),
                    dur: 0,
                    succ: None,
                    stamp: 0,
                };
                (Op::ClipDelete { clip: c.id.clone() }, lop)
            }
            Act::Trim {
                track,
                k,
                frames: d,
                ..
            } => {
                let items = &p.timeline.tracks[track].items;
                let Some(c) = items.get(k % items.len().max(1)) else {
                    return;
                };
                let op = Op::ClipTrim {
                    clip: c.id.clone(),
                    edge: Edge::End,
                    delta: frames(d),
                };
                (
                    op,
                    LOp::Trim {
                        id: c.id.clone(),
                        delta: d,
                    },
                )
            }
            _ => unreachable!(),
        };
        if apply_tx(&p, &tx(vec![op.clone()]), &mut self.log.ids.clone()).is_err() {
            return;
        }
        let id = self.tx();
        let rec = self.log.commit(BR[b], &id, AUTHORS[a], vec![op]);
        if let LOp::Insert { id: clip, .. } = &mut lop {
            *clip = rec.record.bindings["$x"].clone();
        }
        let made = self.m.br[b]
            .run(&lop, false)
            .unwrap_or_else(|e| panic!("the model refuses edit {id}: {e}"));
        self.m.push(
            b,
            ARec {
                tx: id.clone(),
                seq: rec.seq,
                kind: Kind::Edit,
                root: id.clone(),
                ops: vec![made],
            },
        );
        self.m.push_unit(b, a, &id, vec![id.clone()], rec.seq);
    }

    fn undo(&mut self, a: usize, b: usize) -> Result<(), TestCaseError> {
        let id = self.tx();
        let real = self.log.undo(BR[b], AUTHORS[a], &id);
        let Some(unit) = self.m.stacks(b, a).undo.last().cloned() else {
            let e = real.expect_err("the model's undo stack is empty");
            prop_assert_eq!(e.code.as_str(), "NOTHING_TO_UNDO");
            return Ok(());
        };
        let ops = self.m.inverse_of(b, &unit);
        match (self.m.plan_inverse(b, &id, unit.last_seq, &ops), real) {
            (Ok(next), Ok(rec)) => {
                self.m.br[b] = next;
                let last = self.m.br[b].recs.last_mut().unwrap();
                last.seq = rec.seq;
                last.kind = Kind::Undo(unit.id.clone());
                let s = self.m.raw(b, a);
                if let Some(i) = s.undo.iter().rposition(|u| u.id == unit.id) {
                    s.undo.remove(i);
                }
                s.redo.push(Unit {
                    id: id.clone(),
                    records: vec![id],
                    last_seq: rec.seq,
                    redo: None,
                });
            }
            (Err(want), Err(e)) => prop_assert_eq!(preds(&e), vec![want], "undo of {}", unit.id),
            (Ok(_), Err(e)) => prop_assert!(false, "undo of {} refused: {:?}", unit.id, e),
            (Err(want), Ok(_)) => prop_assert!(false, "undo of {} should fail {:?}", unit.id, want),
        }
        Ok(())
    }

    fn redo(&mut self, a: usize, b: usize) -> Result<(), TestCaseError> {
        let id = self.tx();
        let real = self.log.redo(BR[b], AUTHORS[a], &id);
        let Some(undo) = self.m.stacks(b, a).redo.last().cloned() else {
            let e = real.expect_err("the model's redo stack is empty");
            prop_assert_eq!(e.code.as_str(), "NOTHING_TO_REDO");
            return Ok(());
        };
        let ops: Vec<LOp> = self.m.br[b]
            .rec(&undo.id)
            .ops
            .iter()
            .rev()
            .map(LOp::inverse)
            .collect();
        match (self.m.plan_inverse(b, &id, undo.last_seq, &ops), real) {
            (Ok(next), Ok(rec)) => {
                self.m.br[b] = next;
                let last = self.m.br[b].recs.last_mut().unwrap();
                last.seq = rec.seq;
                last.kind = Kind::Redo(undo.id.clone());
                let s = self.m.raw(b, a);
                if let Some(i) = s.redo.iter().rposition(|u| u.id == undo.id) {
                    s.redo.remove(i);
                }
                s.undo.push(Unit {
                    id: id.clone(),
                    records: vec![id.clone()],
                    last_seq: rec.seq,
                    redo: Some(id),
                });
            }
            (Err(want), Err(e)) => prop_assert_eq!(preds(&e), vec![want], "redo of {}", undo.id),
            (Ok(_), Err(e)) => prop_assert!(false, "redo of {} refused: {:?}", undo.id, e),
            (Err(want), Ok(_)) => prop_assert!(false, "redo of {} should fail {:?}", undo.id, want),
        }
        Ok(())
    }

    /// Whether main holds a record of `root`.
    fn present(&self, root: &str) -> bool {
        self.m.br[MAIN].recs.iter().any(|r| r.root == root)
    }

    fn pick(&mut self, a: usize, k: usize) -> Result<(), TestCaseError> {
        let side = self.m.br[SIDE].recs.clone();
        if side.is_empty() {
            return Ok(());
        }
        let i = k % side.len();
        let origin = side[i].clone();
        let id = self.tx();
        let real = self.log.pick(&origin.tx, BR[MAIN], AUTHORS[a], &id);
        if self.present(&origin.root) {
            prop_assert!(
                matches!(real, Ok(None)),
                "{} is on main: {:?}",
                origin.tx,
                real
            );
            return Ok(());
        }
        if let Kind::Undo(u) | Kind::Redo(u) = &origin.kind
            && !self.present(&self.m.br[SIDE].rec(u).root)
        {
            // Named by the picked record, as `UNDOES_TARGET_RECORDS` is.
            let dep = reasons::DEPENDS_ON_SKIPPED.to_string();
            let want = (origin.tx.clone(), 0, dep, Some(u.clone()));
            let e = real.expect_err("a pick of an undo whose unit main does not hold");
            prop_assert_eq!(preds(&e), vec![want]);
            return Ok(());
        }
        let paths = Paths::new(&side, &self.m.br[MAIN].recs);
        let steps = paths.steps(i, &[]);
        let before: HashSet<String> = side[..i].iter().map(|r| r.tx.clone()).collect();
        let mut next = self.m.br[MAIN].clone();
        let planned = rerun(
            &mut next,
            &id,
            &origin.ops,
            &steps,
            true,
            reasons::ALREADY_PRESENT,
        )
        .map_err(|c| on_skipped(c, &before));
        match (planned, real) {
            (Ok(made), Ok(Some(rec))) => {
                self.m.br[MAIN] = next;
                self.m.push(
                    MAIN,
                    ARec {
                        tx: id.clone(),
                        seq: rec.seq,
                        kind: Kind::Pick,
                        root: origin.root.clone(),
                        ops: made,
                    },
                );
                self.m.push_unit(MAIN, a, &id, vec![id.clone()], rec.seq);
            }
            (Err(want), Err(e)) => prop_assert_eq!(preds(&e), vec![want], "pick of {}", origin.tx),
            (want, real) => prop_assert!(
                false,
                "pick of {}: model {:?}, real {:?}",
                origin.tx,
                want,
                real
            ),
        }
        Ok(())
    }

    fn merge(&mut self, a: usize, skip: bool) -> Result<(), TestCaseError> {
        let mid = self.tx();
        let real = self.log.merge(BR[SIDE], BR[MAIN], AUTHORS[a], &mid, skip);
        let side = self.m.br[SIDE].recs.clone();
        let paths = Paths::new(&side, &self.m.br[MAIN].recs);
        let mut next = self.m.br[MAIN].clone();
        let mut copies: Vec<(usize, AStep)> = Vec::new();
        let mut made: Vec<(String, ARec)> = Vec::new();
        let mut conflicts: Vec<Pred> = Vec::new();
        let mut skipped: HashSet<String> = HashSet::new();
        for (i, r) in side.iter().enumerate() {
            if self.present(&r.root) {
                continue;
            }
            if let Kind::Undo(u) | Kind::Redo(u) = &r.kind
                && skipped.contains(u)
            {
                let dep = reasons::DEPENDS_ON_SKIPPED.to_string();
                conflicts.push((r.tx.clone(), 0, dep, Some(u.clone())));
                skipped.insert(r.tx.clone());
                continue;
            }
            let steps = paths.steps(i, &copies);
            let mut scratch = next.clone();
            match rerun(
                &mut scratch,
                &r.tx,
                &r.ops,
                &steps,
                true,
                reasons::ALREADY_PRESENT,
            ) {
                Ok(ops) => {
                    next = scratch;
                    let copy = ARec {
                        tx: format!("{mid}-{}", made.len() + 1),
                        seq: 0,
                        kind: Kind::Merge(mid.clone()),
                        root: r.root.clone(),
                        ops,
                    };
                    copies.push((i, AStep::forward(&copy)));
                    made.push((r.tx.clone(), copy));
                }
                Err(c) => {
                    conflicts.push(on_skipped(c, &skipped));
                    skipped.insert(r.tx.clone());
                }
            }
        }
        let real = match real {
            Ok(plan) => {
                prop_assert!(
                    skip || conflicts.is_empty(),
                    "merge {mid} should fail {conflicts:?}"
                );
                let got: Vec<Pred> = plan.conflicts.into_iter().map(pred).collect();
                prop_assert_eq!(&got, &conflicts, "merge {} conflicts", mid);
                plan.planned
            }
            Err(e) => {
                prop_assert!(!skip, "merge {mid} refused with --skip-conflicting: {e:?}");
                prop_assert_eq!(preds(&e), conflicts, "merge {} conflicts", mid);
                return Ok(());
            }
        };
        prop_assert_eq!(real.len(), made.len(), "merge {} copies", mid);
        let head = self.log.head_seq(BR[MAIN]);
        let first = head + 1 - made.len() as u64;
        let mut group = Vec::new();
        for (n, (origin, mut c)) in made.into_iter().enumerate() {
            c.seq = first + n as u64;
            self.m.merged_away.insert(origin);
            group.push(c.tx.clone());
            next.recs.push(c);
        }
        self.m.br[MAIN] = next;
        if !group.is_empty() {
            self.m.push_unit(MAIN, a, &mid, group, head);
        }
        Ok(())
    }

    /// The real heads against the model, track by track; a track the model is unsure of is
    /// compared as a set and its real order taken. Then the stacks.
    fn check(&mut self, n_authors: usize) -> Result<(), TestCaseError> {
        for (b, br) in BR.iter().enumerate() {
            let p = self.log.state(br);
            assert_well_formed(&p);
            for (t, track) in TRACKS.iter().enumerate() {
                let real: Vec<(String, i64)> = p.timeline.tracks[t]
                    .items
                    .iter()
                    .map(|c| (c.id.clone(), to_frames(c.duration)))
                    .collect();
                let model = &mut self.m.br[b];
                if model.unsure[t] {
                    let mut x = real.clone();
                    let mut y = model.tracks[t].clone();
                    x.sort();
                    y.sort();
                    prop_assert_eq!(x, y, "{} {} as a set", br, track);
                    model.tracks[t] = real;
                    model.unsure[t] = false;
                } else {
                    prop_assert_eq!(&real, &model.tracks[t], "{} {}", br, track);
                }
            }
            for (a, author) in AUTHORS.iter().enumerate().take(n_authors) {
                let real = self.log.stacks(br, author);
                let model = self.m.stacks(b, a);
                let unit_ids = |s: &[Unit]| s.iter().map(|u| u.id.clone()).collect::<Vec<_>>();
                prop_assert_eq!(
                    ids(&real.undo),
                    unit_ids(&model.undo),
                    "{} undo of {}",
                    br,
                    author
                );
                prop_assert_eq!(
                    ids(&real.redo),
                    unit_ids(&model.redo),
                    "{} redo of {}",
                    br,
                    author
                );
            }
        }
        Ok(())
    }
}

proptest! {
    #![proptest_config(prop_config(64))]
    #[test]
    fn undo_redo_matches_reference_model(script in script()) {
        let g = genesis();
        let mut log = MemLog::new(g.clone());
        log.fork(BR[SIDE], BR[MAIN], 0);
        let n_authors = script.iter().map(|(a, _)| a + 1).max().unwrap_or(0);
        let mut run = Run { log, m: Model::new(&g), n: 0 };
        for (a, act) in &script {
            run.step(*a, act)?;
            run.check(n_authors)?;
        }
    }
}

/// The tie-break the model allows (plan 4B Task 12 review): Bob deletes c3, Alice deletes c2,
/// Bob undoes, Alice undoes → `c1, c3, c2`. The recorded successors predict it.
#[test]
fn prosemirror_tie_break_is_the_models_order() {
    let mut log = MemLog::new(with_clips(&[10, 10, 10]));
    log.commit(
        "main",
        "b1",
        "bob",
        vec![Op::ClipDelete { clip: "c3".into() }],
    );
    log.commit(
        "main",
        "a1",
        "alice",
        vec![Op::ClipDelete { clip: "c2".into() }],
    );
    log.undo("main", "bob", "b2").unwrap();
    log.undo("main", "alice", "a2").unwrap();
    let order: Vec<String> = starts(&log.state("main"), 0)
        .into_iter()
        .map(|(id, _)| id)
        .collect();
    assert_eq!(order, ["c1", "c3", "c2"]);
}
