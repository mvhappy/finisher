//! The driver: scripts of acts, run on the real log and the model step by step (module docs
//! in `mod.rs`).

use std::collections::HashSet;

use fin_core::history::UnitEntry;
use fin_core::rebase::{Conflict, reasons};
use fin_core::{CoreError, Edge, InsertMode, ItemSplit, Project, Rational, apply_tx};
use proptest::prelude::*;
use proptest::sample::Index;

use super::branch::{GAP, MBranch};
use super::lops::{LOp, POSITION, UNHELD, Verdict};
use super::model::{ARec, Kind, Model, Unit};
use super::rules::{AStep, Paths, Pred, on_skipped, rerun, takes};
use super::{AUTHORS, BR, MAIN, SIDE, TRACKS, frames, to_frames};
use crate::common::memlog::MemLog;
use crate::common::moves::move_to;
use crate::common::{assert_well_formed, delete, insert, secs, trim, tx};

#[derive(Debug, Clone)]
pub enum Act {
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
    /// A ripple move of clip `k` of `track` to track `to`, before its clip `at` (or at its
    /// end), positions in the frame before the move.
    Move {
        side: bool,
        track: usize,
        k: usize,
        to: usize,
        at: usize,
    },
    /// A split of clip `k` of `track` at a whole frame strictly inside it (`off` mod its
    /// length less one, plus one).
    Split {
        side: bool,
        track: usize,
        k: usize,
        off: usize,
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

/// The 4B act mix (its seeds replay only with exactly this strategy).
pub fn act_4b() -> BoxedStrategy<Act> {
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
    .boxed()
}

/// The 4C act mix: the 4B one, with cross-track and same-track ripple moves and splits.
pub fn act_4c() -> BoxedStrategy<Act> {
    prop_oneof![
        3 => (any::<bool>(), 0usize..2, 0usize..5, 5i64..40)
            .prop_map(|(side, track, at, len)| Act::Insert { side, track, at, len }),
        2 => (any::<bool>(), 0usize..2, 0usize..5)
            .prop_map(|(side, track, k)| Act::Delete { side, track, k }),
        3 => (any::<bool>(), 0usize..2, 0usize..5, prop_oneof![-9i64..=-1, 1i64..=9])
            .prop_map(|(side, track, k, frames)| Act::Trim { side, track, k, frames }),
        3 => (any::<bool>(), 0usize..2, 0usize..5, 0usize..2, 0usize..6)
            .prop_map(|(side, track, k, to, at)| Act::Move { side, track, k, to, at }),
        2 => (any::<bool>(), 0usize..2, 0usize..5, any::<usize>())
            .prop_map(|(side, track, k, off)| Act::Split { side, track, k, off }),
        3 => any::<bool>().prop_map(|side| Act::Undo { side }),
        2 => any::<bool>().prop_map(|side| Act::Redo { side }),
        1 => (0usize..16).prop_map(|k| Act::Pick { k }),
        2 => (any::<bool>(), any::<bool>()).prop_map(|(skip, down)| Act::Merge { skip, down }),
    ]
    .boxed()
}

/// 2–4 authors with 0–12 acts each from `act`, interleaved: `(author, act)` in the order they
/// run.
pub fn script(act: fn() -> BoxedStrategy<Act>) -> impl Strategy<Value = Vec<(usize, Act)>> {
    (2usize..=4)
        .prop_flat_map(move |n| {
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

/// Runs `script` from `genesis`: after every step the heads, the stacks and every conflict
/// reason match the model. Stops early at a state the model does not follow (a gap, module
/// docs); returns how many steps ran.
pub fn run_script(genesis: &Project, script: &[(usize, Act)]) -> Result<usize, TestCaseError> {
    let mut log = MemLog::new(genesis.clone());
    log.fork(BR[SIDE], BR[MAIN], 0);
    let n_authors = script.iter().map(|(a, _)| a + 1).max().unwrap_or(0);
    let mut run = Run {
        log,
        m: Model::new(genesis),
        n: 0,
        stopped: false,
    };
    for (i, (a, act)) in script.iter().enumerate() {
        run.step(*a, act)?;
        run.check(n_authors)?;
        if run.stopped {
            return Ok(i + 1);
        }
    }
    Ok(script.len())
}

/// The independent oracle against a real answer of one record: where it says op `k` cannot
/// be carried, the real answer is a conflict, with a reason it allows when it names op `k`.
fn agree<T: std::fmt::Debug>(
    verdict: Verdict,
    real: &Result<T, CoreError>,
    what: &str,
) -> Result<(), TestCaseError> {
    let Some((k, class, risky)) = verdict else {
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
        prop_assert!(allows(class, risky, &c.2), "{what}: {c:?} not in {class:?}");
    }
    Ok(())
}

fn allows(class: &[&str], risky: bool, reason: &str) -> bool {
    class.contains(&reason) || risky && POSITION.contains(&reason)
}

struct Run {
    log: MemLog,
    m: Model,
    n: usize,
    stopped: bool,
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

/// A refusal of one record against the model's answer: equal to the predicted conflict, or
/// one the model takes as it is ([`takes`]).
fn refused(
    want: Result<(), Pred>,
    risky: &[usize],
    e: &CoreError,
    what: &str,
) -> Result<(), TestCaseError> {
    let got = preds(e);
    if want.as_ref().err().is_some_and(|w| vec![w.clone()] == got) {
        return Ok(());
    }
    if got.len() == 1 && takes(risky, want.as_ref().err(), &got[0]) {
        return Ok(());
    }
    match want {
        Err(want) => prop_assert_eq!(got, vec![want], "{}", what),
        Ok(()) => prop_assert!(false, "{what} refused: {e:?}"),
    }
    Ok(())
}

/// Whether the items of `track` follow each other from 0 with no gap.
fn gapless(p: &Project, t: usize) -> bool {
    let mut end = Rational::zero();
    for c in &p.timelines[0].tracks[t].items {
        if c.start != end {
            return false;
        }
        end = (c.start + c.duration).unwrap();
    }
    true
}

impl Run {
    fn tx(&mut self) -> String {
        self.n += 1;
        format!("x{}", self.n)
    }

    fn step(&mut self, a: usize, act: &Act) -> Result<(), TestCaseError> {
        match *act {
            Act::Insert { side, .. }
            | Act::Delete { side, .. }
            | Act::Trim { side, .. }
            | Act::Move { side, .. }
            | Act::Split { side, .. } => {
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
    /// skipped when it does not apply there (a trim past the limits, an empty track, a clip
    /// too short to split).
    fn edit(&mut self, a: usize, b: usize, act: &Act) {
        let p = self.log.state(BR[b]);
        let items = |t: usize| &p.timelines[0].tracks[t].items;
        let pick = |t: usize, k: usize| items(t).get(k % items(t).len().max(1));
        let mb: &MBranch = &self.m.br[b];
        let (op, mut lop) = match *act {
            Act::Insert { track, at, len, .. } => {
                let (pos, before) = Self::target(&p, track, at);
                let stamp = mb.removals(before.as_ref());
                let op = insert("$x", TRACKS[track], pos, "a1", secs(1), frames(len));
                let lop = LOp::Insert {
                    track,
                    id: String::new(),
                    dur: len,
                    before,
                    stamp,
                    ns: mb.ns[track],
                    expect: None,
                    pre: Vec::new(),
                };
                (op, lop)
            }
            Act::Delete { track, k, .. } => {
                let Some(c) = pick(track, k) else { return };
                let lop = LOp::Remove {
                    track,
                    id: c.id.clone(),
                    dur: 0,
                    succ: None,
                    stamp: 0,
                    ns: 0,
                    post: Vec::new(),
                };
                (delete(&c.id), lop)
            }
            Act::Trim {
                track,
                k,
                frames: d,
                ..
            } => {
                let Some(c) = pick(track, k) else { return };
                let op = trim(&c.id, Edge::End, frames(d));
                let lop = LOp::Trim {
                    id: c.id.clone(),
                    delta: d,
                    strict: false,
                    eff: String::new(),
                    push: true,
                    expect: None,
                    pre: Vec::new(),
                    post: Vec::new(),
                };
                (op, lop)
            }
            Act::Move {
                track, k, to, at, ..
            } => {
                let Some(c) = pick(track, k) else { return };
                let (pos, before) = Self::target(&p, to, at);
                let op = move_to(&c.id, TRACKS[to], pos, InsertMode::Ripple);
                let lop = LOp::Move {
                    id: c.id.clone(),
                    to,
                    stamp: mb.removals(before.as_ref()),
                    before,
                    ns: mb.ns[to],
                    from: track,
                    dur: 0,
                    succ: None,
                    sstamp: 0,
                    sns: 0,
                    post: Vec::new(),
                };
                (op, lop)
            }
            Act::Split { track, k, off, .. } => {
                let Some(c) = pick(track, k) else { return };
                let dur = to_frames(c.duration);
                if dur < 2 {
                    return;
                }
                let off = 1 + (off % (dur as usize - 1)) as i64;
                let op = ItemSplit {
                    item: c.id.clone(),
                    offset: frames(off),
                    id: "$x".into(),
                }
                .into();
                let lop = LOp::Split {
                    id: c.id.clone(),
                    right: String::new(),
                    off,
                    rlen: 0,
                    eff: String::new(),
                    pre: Vec::new(),
                    post: Vec::new(),
                };
                (op, lop)
            }
            _ => unreachable!(),
        };
        if apply_tx(&p, &tx(vec![op.clone()]), &mut self.log.ids.clone()).is_err() {
            return;
        }
        let id = self.tx();
        let rec = self.log.commit(BR[b], &id, AUTHORS[a], vec![op]);
        match &mut lop {
            LOp::Trim { eff, .. } => *eff = id.clone(),
            LOp::Insert { id: clip, .. } => *clip = rec.record.bindings["$x"].clone(),
            LOp::Split { right, eff, .. } => {
                *right = rec.record.bindings["$x"].clone();
                *eff = id.clone();
            }
            _ => {}
        }
        let made = self.m.br[b]
            .run(&lop, false)
            .unwrap_or_else(|e| panic!("the model refuses edit {id}: {e:?}"));
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

    /// Where an insert or move to clip `at` of `track` goes in `p`: that clip's start, or the
    /// track end; and the clip it goes before.
    fn target(p: &Project, track: usize, at: usize) -> (Rational, Option<String>) {
        let items = &p.timelines[0].tracks[track].items;
        match items.get(at) {
            Some(c) => (c.start, Some(c.id.clone())),
            None => (
                items
                    .last()
                    .map_or(Rational::zero(), |c| (c.start + c.duration).unwrap()),
                None,
            ),
        }
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
        let planned = self.m.plan_inverse(b, &id, unit.last_seq, &ops);
        let what = format!("undo of {}", unit.id);
        agree(planned.verdict, &real, &what)?;
        match (planned.out, real) {
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
            (want, Err(e)) => refused(want.map(|_| ()), &planned.risky, &e, &what)?,
            (Err(want), Ok(_)) => prop_assert!(false, "{what} should fail {want:?}"),
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
            .flat_map(LOp::inverse)
            .collect();
        let planned = self.m.plan_inverse(b, &id, undo.last_seq, &ops);
        let what = format!("redo of {}", undo.id);
        agree(planned.verdict, &real, &what)?;
        match (planned.out, real) {
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
            (want, Err(e)) => refused(want.map(|_| ()), &planned.risky, &e, &what)?,
            (Err(want), Ok(_)) => prop_assert!(false, "{what} should fail {want:?}"),
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
        let refuse = if self.m.undoes_target(MAIN, &origin, 0) {
            Some((reasons::UNDOES_TARGET_RECORDS, None))
        } else {
            self.unit_dep(SIDE, &origin, |x| !self.m.holds(MAIN, &x.root))
                .map(|u| (reasons::DEPENDS_ON_SKIPPED, Some(u)))
        };
        if let Some((reason, against)) = refuse {
            let want = (origin.tx.clone(), 0, reason.to_string(), against);
            let e = real.expect_err("a pick of an undo tied to main or to what main lacks");
            prop_assert_eq!(preds(&e), vec![want]);
            return Ok(());
        }
        let paths = Paths::new(&side, &self.m.br[MAIN].recs);
        let steps = paths.steps(i, &[]);
        let before: HashSet<String> = side[..i].iter().map(|r| r.tx.clone()).collect();
        let mut next = self.m.br[MAIN].clone();
        let r = rerun(
            &mut next,
            &id,
            &origin.ops,
            &steps,
            true,
            reasons::ALREADY_PRESENT,
        );
        let planned = r.out.map_err(|c| on_skipped(c, &before));
        let what = format!("pick of {}", origin.tx);
        agree(r.verdict, &real, &what)?;
        let real = real.map(|r| r.expect("a pick of a record main lacks"));
        match (planned, real) {
            (Ok(made), Ok(rec)) => {
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
            (want, Err(e)) => refused(want.map(|_| ()), &r.risky, &e, &what)?,
            (Err(want), Ok(_)) => prop_assert!(false, "{what} should fail {want:?}"),
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
        let got: Vec<Pred> = match &real {
            Ok(plan) => plan.conflicts.iter().cloned().map(pred).collect(),
            Err(e) => preds(e),
        };
        let source = self.m.br[from].recs.clone();
        let paths = Paths::new(&source, &self.m.br[into].recs);
        let mut next = self.m.br[into].clone();
        let mut copies: Vec<(usize, AStep)> = Vec::new();
        let mut made: Vec<(String, ARec)> = Vec::new();
        let (mut conflicts, mut excluded): (Vec<Pred>, Vec<Pred>) = (Vec::new(), Vec::new());
        // Candidates the independent oracle says cannot be copied: (txId, op, reasons, risky).
        let mut must_fail: Vec<(String, usize, &[&str], bool)> = Vec::new();
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
                must_fail.push((r.tx.clone(), 0, UNHELD, false));
                let why = reasons::DEPENDS_ON_SKIPPED.to_string();
                conflicts.push((r.tx.clone(), 0, why, Some(dep)));
                skipped.insert(r.tx.clone());
                continue;
            }
            let steps = paths.steps(i, &copies);
            let mut scratch = next.clone();
            let out = rerun(
                &mut scratch,
                &r.tx,
                &r.ops,
                &steps,
                true,
                reasons::ALREADY_PRESENT,
            );
            if let Some((k, class, risky)) = out.verdict {
                must_fail.push((r.tx.clone(), k, class, risky));
            }
            if let Some(c) = got.iter().find(|c| c.0 == r.tx)
                && out.takes(c)
            {
                conflicts.push(c.clone());
                skipped.insert(r.tx.clone());
                continue;
            }
            match out.out {
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
        for (tx, k, class, risky) in &must_fail {
            let c = got.iter().find(|c| c.0 == *tx);
            prop_assert!(
                c.is_some(),
                "merge {mid}: {tx} op {k} cannot be copied ({class:?})"
            );
            let c = c.unwrap();
            prop_assert!(
                c.1 != *k || allows(class, *risky, &c.2),
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

    /// The real heads against the model, track by track: equal lists with no gap; a track the
    /// model is unsure of is compared as a set and its real order taken. A gap (predicted, or
    /// real on a track the model is unsure of) stops the run there (module docs). Then the
    /// stacks.
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
                let whole = gapless(&p, t);
                let model = &mut self.m.br[b];
                let gap = model.has_gap(t);
                let mut mine: Vec<(String, i64)> = model.tracks[t]
                    .iter()
                    .filter(|c| c.0 != GAP)
                    .cloned()
                    .collect();
                if model.unsure[t] {
                    let mut x = real.clone();
                    x.sort();
                    mine.sort();
                    prop_assert_eq!(x, mine, "{} {} as a set", br, track);
                } else {
                    prop_assert_eq!(&real, &mine, "{} {}", br, track);
                }
                if !model.unsure[t] {
                    prop_assert_eq!(!whole, gap, "{} {}: a gap", br, track);
                }
                self.stopped |= !whole;
                model.tracks[t] = real;
                model.unsure[t] = false;
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
