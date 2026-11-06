//! Undo/redo property (spec §4.6 prop 4; plan 4B Task 20): 2–4 authors interleave edits,
//! undos, redos, cherry-picks (side branch → main) and merges (side branch → main, and main →
//! side branch, so a merge can carry copies back and exclude undos of them) on `main` and a
//! side branch `br1`, through the real planners over a [`MemLog`]. After every step the
//! heads, the stacks and every conflict reason match a reference model.
//!
//! **Edits** are gapless ripple edits: inserts at a clip boundary or at the track end, deletes
//! and end trims, one op each. A track is then exactly its ordered list of `(clipId,
//! duration)`.
//!
//! **The model** knows nothing of positions. It keeps:
//! - per branch, the ordered clip lists, changed by list operations: insert an id before its
//!   recorded successor id (or at the end), remove an id, add to a duration;
//! - per branch and clip, the end trims in effect (each by the edit whose effect it is: a copy
//!   keeps it, an undo takes it back);
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
//! **Two oracles for conflicts.**
//! - *Independent* ([`MBranch::verdict`]): from the clip lists and edge histories alone, an
//!   op cannot be carried when its clip is gone (remove, trim), is there already (a restore),
//!   or its end edge was changed by someone else since (an undo or redo: since the edit it
//!   inverts; a copy: since what the source had before it); and a copied undo or redo cannot be
//!   carried when the target does not hold its unit. Wherever it says so, the real planner
//!   must refuse, with a reason of that kind. It allows the planner to refuse more (it is
//!   conservative: two unmirrored records may cancel in effect), so it cannot predict every
//!   conflict.
//! - *Transcribed* (shared with the implementation's rules, so a bug in those rules could be
//!   shared too): the exact reason and blamed record come from abstract footprints (a restore
//!   creates its clip, a removal needs and removes it, a trim needs its end edge) over the
//!   records between a unit and the head, or over a copy's merge steps (the source records
//!   before it inverted, the target's records, the copies so far), paired as Decision 24 says
//!   (an undo with its units, a redo with its undo, a copy with its inverted original, a target
//!   record with the inverted source record of the same origin root), plus the candidate rules
//!   (present by root; `UNDOES_TARGET_RECORDS`; an undo or redo of a skipped unit is
//!   `DEPENDS_ON_SKIPPED`, found by this property: a skipped delete's undo re-created a clip
//!   the target had deleted). The heads, which the transcribed part never computes, are
//!   checked by the list model alone.
//!
//! Not driven here: `revert` (its chains have their own oracle, `pick_props`), `undo --to`
//! (`undo_to_props`: equal to the undos in a row), and lineages deeper than one fork
//! (`merge.rs` cases; the store's prop 5, `props_replay`, drives all of them end to end).

mod common;

use std::collections::{BTreeMap, HashMap, HashSet};

use common::memlog::MemLog;
use common::*;
use fin_core::history::UnitEntry;
use fin_core::rebase::{Conflict, reasons};
use fin_core::{CoreError, Edge, Project, Rational, apply_tx};
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
    /// A merge of the side branch into main, or of main into the side branch (`down`).
    Merge {
        skip: bool,
        down: bool,
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
        2 => (any::<bool>(), any::<bool>()).prop_map(|(skip, down)| Act::Merge { skip, down }),
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
    /// An end trim. `eff`: the edit whose effect it is (a copy keeps it, an undo takes it
    /// back: `push` false); `expect`: the edge history it needs (an undo or redo: as the edit
    /// it inverts left it); `pre`/`post`: the history around it as made (the independent
    /// oracle, module docs).
    Trim {
        id: String,
        delta: i64,
        eff: String,
        push: bool,
        expect: Option<Vec<String>>,
        pre: Vec<String>,
        post: Vec<String>,
    },
}

const UNSURE: u32 = u32::MAX;

/// What the engine may answer when the independent oracle says an op is impossible. A clip
/// gone may also be the re-run's own `NOT_FOUND` (Decision 17): a copy's entity judgement
/// leaves out mirrored pairs, and when one pair's two steps lie around another removal of the
/// clip (an inverted source undo that `UNDOES_TARGET_RECORDS` excluded), the judgement misses
/// that the clip is gone (Task 22 follow-up: name the excluded record instead).
const GONE: &[&str] = &[reasons::REMOVED, reasons::DEPENDS_ON_SKIPPED, "NOT_FOUND"];
const THERE: &[&str] = &[
    reasons::ALREADY_PRESENT,
    reasons::ID_COLLISION,
    reasons::DEPENDS_ON_SKIPPED,
];
const EDGE: &[&str] = &[
    reasons::SAME_EDGE,
    reasons::REMOVED,
    reasons::DEPENDS_ON_SKIPPED,
];
const UNHELD: &[&str] = &[reasons::DEPENDS_ON_SKIPPED];

/// An independent verdict: op `k` cannot be carried, and the reasons the engine may give.
type Verdict = Option<(usize, &'static [&'static str])>;

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
            LOp::Trim {
                id,
                delta,
                eff,
                push,
                post,
                ..
            } => LOp::Trim {
                id,
                delta: -delta,
                eff,
                push: !push,
                expect: Some(post),
                pre: Vec::new(),
                post: Vec::new(),
            },
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
    /// Per clip, the trims of its end in effect, oldest first (by `eff`).
    edges: HashMap<String, Vec<String>>,
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

    /// The independent oracle for `op` here: the reasons the engine may give when the op
    /// cannot be carried, from what the clip lists and edge histories say alone (no steps, no
    /// mirrors). A clip to remove or trim is gone; a clip to put back is there; an end edge
    /// someone else changed since (an undo or redo: since the edit it inverts; a copy: since
    /// what the source had before it).
    fn verdict(&self, op: &LOp, copy: bool) -> Option<&'static [&'static str]> {
        let present = self.find(op.clip()).is_some();
        match op {
            LOp::Insert { .. } => present.then_some(THERE),
            LOp::Remove { .. } => (!present).then_some(GONE),
            LOp::Trim { .. } if !present => Some(GONE),
            LOp::Trim {
                id, expect, pre, ..
            } => {
                let want = if copy { Some(pre) } else { expect.as_ref() };
                let here = self.edges.get(id).cloned().unwrap_or_default();
                want.is_some_and(|w| *w != here).then_some(EDGE)
            }
        }
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
            LOp::Trim {
                id,
                delta,
                eff,
                push,
                expect,
                ..
            } => {
                let (t, i) = self.find(&id).ok_or("NOT_FOUND")?;
                let d = self.tracks[t][i].1 + delta;
                if !(1..=MAX_FRAMES).contains(&d) {
                    return Err("INVALID_RANGE");
                }
                self.tracks[t][i].1 = d;
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
                    eff,
                    push,
                    expect,
                    pre,
                    post: hist.clone(),
                })
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

/// `ops` re-run one by one on `b` over `steps` (module docs): the ops as made, or the conflict
/// the transcribed rules predict; and the independent oracle's verdict on the ops it reached.
/// `dup`: the reason a re-created clip that is there already gets (a copy's same origin:
/// `ALREADY_PRESENT`; an undo's: `ID_COLLISION`).
fn rerun(
    b: &mut MBranch,
    tx: &str,
    ops: &[LOp],
    steps: &[AStep],
    copy: bool,
    dup: &'static str,
) -> (Result<Vec<LOp>, Pred>, Verdict) {
    let mut verdict = None;
    let out = rerun_ops(b, tx, ops, steps, copy, dup, &mut verdict);
    (out, verdict)
}

fn rerun_ops(
    b: &mut MBranch,
    tx: &str,
    ops: &[LOp],
    steps: &[AStep],
    copy: bool,
    dup: &'static str,
    verdict: &mut Verdict,
) -> Result<Vec<LOp>, Pred> {
    let theirs = unmirrored(steps);
    let mut created = HashSet::new();
    let mut made = Vec::new();
    for (k, op) in ops.iter().enumerate() {
        if verdict.is_none() {
            *verdict = b.verdict(op, copy).map(|class| (k, class));
        }
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
            b.tracks[t] = p.timelines[0].tracks[t]
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

    /// Plans `ops` on `b` over its records after `at` (an undo or a redo), with the
    /// independent verdict.
    fn plan_inverse(
        &self,
        b: usize,
        tx: &str,
        at: u64,
        ops: &[LOp],
    ) -> (Result<MBranch, Pred>, Verdict) {
        let steps = steps_from(&self.after(b, at));
        let mut next = self.br[b].clone();
        let (made, verdict) = rerun(&mut next, tx, ops, &steps, false, reasons::ID_COLLISION);
        let out = made.map(|made| {
            next.recs.push(ARec {
                tx: tx.to_string(),
                seq: 0,
                kind: Kind::Edit,
                root: tx.to_string(),
                ops: made,
            });
            next
        });
        (out, verdict)
    }

    /// The record `tx`, on whichever branch holds it.
    fn find(&self, tx: &str) -> Option<&ARec> {
        self.br.iter().flat_map(|b| &b.recs).find(|r| r.tx == tx)
    }

    /// The records of unit `u` on `b`: the record `u`, or the merge group `u`.
    fn unit_recs(&self, b: usize, u: &str) -> Vec<&ARec> {
        self.br[b]
            .recs
            .iter()
            .filter(|r| r.tx == u || r.kind == Kind::Merge(u.to_string()))
            .collect()
    }

    /// Whether branch `b` holds a record of `root` (its own records: every lineage here
    /// starts at main's seq 0, which holds no record).
    fn holds(&self, b: usize, root: &str) -> bool {
        self.br[b].recs.iter().any(|r| r.root == root)
    }

    /// Whether `r` (by its root record) is an undo or redo whose units hold a record that
    /// originates on `into`, directly or through another such record
    /// (`UNDOES_TARGET_RECORDS`).
    fn undoes_target(&self, into: usize, r: &ARec, depth: usize) -> bool {
        let base = self.find(&r.root).unwrap_or(r);
        let (Kind::Undo(u) | Kind::Redo(u)) = &base.kind else {
            return false;
        };
        let Some(b) = (0..2).find(|&b| self.br[b].recs.iter().any(|x| x.tx == base.tx)) else {
            return false;
        };
        depth < 8
            && self.unit_recs(b, u).iter().any(|x| {
                let root = self.find(&x.root).unwrap_or(x);
                self.br[into].recs.iter().any(|y| y.tx == root.tx)
                    || self.undoes_target(into, root, depth + 1)
            })
    }
}

/// The independent oracle against a real answer of one record: where it says op `k` cannot
/// be carried, the real answer is a conflict, with a reason it allows when it names op `k`.
fn agree<T: std::fmt::Debug>(
    verdict: Verdict,
    real: &Result<T, CoreError>,
    what: &str,
) -> Result<(), TestCaseError> {
    let Some((k, class)) = verdict else {
        return Ok(());
    };
    let Err(e) = real else {
        prop_assert!(
            false,
            "{what}: op {k} cannot be carried ({class:?}), real {real:?}"
        );
        return Ok(());
    };
    for c in preds(e).iter().filter(|c| c.1 == k) {
        prop_assert!(
            class.contains(&c.2.as_str()),
            "{what}: {c:?} not in {class:?}"
        );
    }
    Ok(())
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
            Act::Merge { skip, down: false } => self.merge(a, SIDE, MAIN, skip),
            Act::Merge { skip, down: true } => self.merge(a, MAIN, SIDE, skip),
        }
    }

    /// An edit at the head: the op from the real state (which the model's order matches),
    /// skipped when it does not apply there (a trim past the limits, an empty track).
    fn edit(&mut self, a: usize, b: usize, act: &Act) {
        let p = self.log.state(BR[b]);
        let (op, mut lop) = match *act {
            Act::Insert { track, at, len, .. } => {
                let items = &p.timelines[0].tracks[track].items;
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
                let items = &p.timelines[0].tracks[track].items;
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
                (delete(&c.id), lop)
            }
            Act::Trim {
                track,
                k,
                frames: d,
                ..
            } => {
                let items = &p.timelines[0].tracks[track].items;
                let Some(c) = items.get(k % items.len().max(1)) else {
                    return;
                };
                let op = trim(&c.id, Edge::End, frames(d));
                (
                    op,
                    LOp::Trim {
                        id: c.id.clone(),
                        delta: d,
                        eff: String::new(),
                        push: true,
                        expect: None,
                        pre: Vec::new(),
                        post: Vec::new(),
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
        if let LOp::Trim { eff, .. } = &mut lop {
            *eff = id.clone();
        }
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
        let (planned, verdict) = self.m.plan_inverse(b, &id, unit.last_seq, &ops);
        agree(verdict, &real, &format!("undo of {}", unit.id))?;
        match (planned, real) {
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
        let (planned, verdict) = self.m.plan_inverse(b, &id, undo.last_seq, &ops);
        agree(verdict, &real, &format!("redo of {}", undo.id))?;
        match (planned, real) {
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

    /// The first record of the unit an undo or redo `r` on `b` addresses (oldest first) for
    /// which `missing` holds.
    fn unit_dep(&self, b: usize, r: &ARec, missing: impl Fn(&ARec) -> bool) -> Option<String> {
        let (Kind::Undo(u) | Kind::Redo(u)) = &r.kind else {
            return None;
        };
        let recs = self.m.unit_recs(b, u);
        recs.into_iter().find(|x| missing(x)).map(|x| x.tx.clone())
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
        if self.m.holds(MAIN, &origin.root) {
            prop_assert!(
                matches!(real, Ok(None)),
                "{} is on main: {:?}",
                origin.tx,
                real
            );
            return Ok(());
        }
        // Both named by the picked record. The second is also the independent oracle's: the
        // undo's unit is not on main, so its inverse would take back what main never got.
        let refused = if self.m.undoes_target(MAIN, &origin, 0) {
            Some((reasons::UNDOES_TARGET_RECORDS, None))
        } else {
            self.unit_dep(SIDE, &origin, |x| !self.m.holds(MAIN, &x.root))
                .map(|u| (reasons::DEPENDS_ON_SKIPPED, Some(u)))
        };
        if let Some((reason, against)) = refused {
            let want = (origin.tx.clone(), 0, reason.to_string(), against);
            let e = real.expect_err("a pick of an undo tied to main or to what main lacks");
            prop_assert_eq!(preds(&e), vec![want]);
            return Ok(());
        }
        let paths = Paths::new(&side, &self.m.br[MAIN].recs);
        let steps = paths.steps(i, &[]);
        let before: HashSet<String> = side[..i].iter().map(|r| r.tx.clone()).collect();
        let mut next = self.m.br[MAIN].clone();
        let (planned, verdict) = rerun(
            &mut next,
            &id,
            &origin.ops,
            &steps,
            true,
            reasons::ALREADY_PRESENT,
        );
        let planned = planned.map_err(|c| on_skipped(c, &before));
        agree(verdict, &real, &format!("pick of {}", origin.tx))?;
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

    /// A merge of `from` into `into` (the side branch into main, or main into the side
    /// branch: both lineages start at main's seq 0, so both paths are whole).
    fn merge(
        &mut self,
        a: usize,
        from: usize,
        into: usize,
        skip: bool,
    ) -> Result<(), TestCaseError> {
        let mid = self.tx();
        let real = self.log.merge(BR[from], BR[into], AUTHORS[a], &mid, skip);
        let source = self.m.br[from].recs.clone();
        let paths = Paths::new(&source, &self.m.br[into].recs);
        let mut next = self.m.br[into].clone();
        let mut copies: Vec<(usize, AStep)> = Vec::new();
        let mut made: Vec<(String, ARec)> = Vec::new();
        let (mut conflicts, mut excluded): (Vec<Pred>, Vec<Pred>) = (Vec::new(), Vec::new());
        // Candidates the independent oracle says cannot be copied: (txId, op, reasons).
        let mut must_fail: Vec<(String, usize, &[&str])> = Vec::new();
        let mut skipped: HashSet<String> = HashSet::new();
        for (i, r) in source.iter().enumerate() {
            if self.m.holds(into, &r.root) {
                continue;
            }
            if self.m.undoes_target(into, r, 0) {
                let why = reasons::UNDOES_TARGET_RECORDS.to_string();
                excluded.push((r.tx.clone(), 0, why, None));
                skipped.insert(r.tx.clone());
                continue;
            }
            // An undo or redo of a unit this merge does not copy (also the oracle's rule).
            if let Some(dep) = self.unit_dep(from, r, |x| skipped.contains(&x.tx)) {
                must_fail.push((r.tx.clone(), 0, UNHELD));
                let why = reasons::DEPENDS_ON_SKIPPED.to_string();
                conflicts.push((r.tx.clone(), 0, why, Some(dep)));
                skipped.insert(r.tx.clone());
                continue;
            }
            let steps = paths.steps(i, &copies);
            let mut scratch = next.clone();
            let (out, verdict) = rerun(
                &mut scratch,
                &r.tx,
                &r.ops,
                &steps,
                true,
                reasons::ALREADY_PRESENT,
            );
            if let Some((k, class)) = verdict {
                must_fail.push((r.tx.clone(), k, class));
            }
            match out {
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
        let got: Vec<Pred> = match &real {
            Ok(plan) => plan.conflicts.iter().cloned().map(pred).collect(),
            Err(e) => preds(e),
        };
        for (tx, k, class) in &must_fail {
            let c = got.iter().find(|c| c.0 == *tx);
            prop_assert!(
                c.is_some(),
                "merge {mid}: {tx} op {k} cannot be copied ({class:?})"
            );
            let c = c.unwrap();
            prop_assert!(
                c.1 != *k || class.contains(&c.2.as_str()),
                "merge {mid}: {c:?}"
            );
        }
        prop_assert_eq!(&got, &conflicts, "merge {} conflicts", mid);
        let real = match real {
            Ok(plan) => {
                prop_assert!(
                    skip || conflicts.is_empty(),
                    "merge {mid} should fail {conflicts:?}"
                );
                let ex: Vec<Pred> = plan.excluded.into_iter().map(pred).collect();
                prop_assert_eq!(ex, excluded, "merge {} excluded", mid);
                plan.planned
            }
            Err(e) => {
                prop_assert!(!skip, "merge {mid} refused with --skip-conflicting: {e:?}");
                return Ok(());
            }
        };
        prop_assert_eq!(real.len(), made.len(), "merge {} copies", mid);
        let head = self.log.head_seq(BR[into]);
        let first = head + 1 - made.len() as u64;
        let mut group = Vec::new();
        for (n, (origin, mut c)) in made.into_iter().enumerate() {
            c.seq = first + n as u64;
            // Decision 22: a merge into a non-descendant drops the source's stacks there.
            if into == MAIN {
                self.m.merged_away.insert(origin);
            }
            group.push(c.tx.clone());
            next.recs.push(c);
        }
        self.m.br[into] = next;
        if !group.is_empty() {
            self.m.push_unit(into, a, &mid, group, head);
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
                let real: Vec<(String, i64)> = p.timelines[0].tracks[t]
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
    log.commit("main", "b1", "bob", vec![delete("c3")]);
    log.commit("main", "a1", "alice", vec![delete("c2")]);
    log.undo("main", "bob", "b2").unwrap();
    log.undo("main", "alice", "a2").unwrap();
    let order: Vec<String> = starts(&log.state("main"), 0)
        .into_iter()
        .map(|(id, _)| id)
        .collect();
    assert_eq!(order, ["c1", "c3", "c2"]);
}
