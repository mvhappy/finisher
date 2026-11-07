//! The transcribed rules: abstract steps, mirrors and the entity rules (Decision 24, spec
//! §4.3), and the re-run of a record's ops over them (module docs in `mod.rs`).

use std::collections::{HashMap, HashSet};

use fin_core::rebase::reasons;

use super::branch::MBranch;
use super::lops::{Fp, LOp, T, Verdict};
use super::model::{ARec, Kind};

#[derive(Debug, Clone)]
pub struct AStep {
    pub tx: String,
    pub fps: Vec<Fp>,
    /// How many ops its inverse holds (`Step::inverse_len`: what an undo's part of it holds).
    pub inv_len: usize,
    pub mirror_of: Option<usize>,
    /// The record whose edit this step applies again (`false`) or takes back (`true`).
    pub unit: Option<(String, bool)>,
}

impl AStep {
    pub fn forward(r: &ARec) -> AStep {
        AStep {
            tx: r.tx.clone(),
            fps: r.ops.iter().map(LOp::fp).collect(),
            inv_len: r.ops.iter().map(LOp::inverse_len).sum(),
            mirror_of: None,
            unit: Some((r.tx.clone(), false)),
        }
    }

    pub fn inverted(r: &ARec) -> AStep {
        let mut fps: Vec<Fp> = r.ops.iter().map(LOp::fp).collect();
        fps.reverse();
        for f in &mut fps {
            std::mem::swap(&mut f.creates, &mut f.removes);
        }
        AStep {
            tx: r.tx.clone(),
            fps,
            inv_len: r.ops.len(),
            mirror_of: None,
            unit: None,
        }
    }
}

/// `recs` as steps with their mirrors (`rebase::steps_with_units`): an undo is cut into one
/// part per step of the units it undoes (each unit a record, or a merge group by its id), a
/// redo into the parts of its undo, each part as long as its step's inverse (a move or a split
/// inverts to two ops) and mirroring a step not mirrored yet; one addressed step needs no cut.
pub fn steps_from(recs: &[&ARec]) -> Vec<AStep> {
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
        .filter(|t| {
            let fits = t.iter().map(|&j| steps[j].inv_len).sum::<usize>() == r.ops.len();
            fits || t.len() == 1 && !r.ops.is_empty()
        });
        let start = steps.len();
        match addressed {
            Some(t) => {
                let sizes: Vec<usize> = if t.len() == 1 {
                    vec![r.ops.len()]
                } else {
                    t.iter().map(|&j| steps[j].inv_len).collect()
                };
                let mut fps = r.ops.iter().map(LOp::fp);
                for (j, size) in t.into_iter().zip(sizes) {
                    let part: Vec<Fp> = fps.by_ref().take(size).collect();
                    let mirror = (!paired[j]).then_some(j);
                    if mirror.is_some() {
                        paired[j] = true;
                    }
                    let unit = steps[j].unit.clone().map(|(b, inv)| (b, !inv));
                    paired.push(mirror.is_some());
                    steps.push(AStep {
                        tx: r.tx.clone(),
                        inv_len: part.len(),
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

pub fn paired(steps: &[AStep]) -> Vec<bool> {
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

fn reason_of(t: T) -> &'static str {
    match t {
        T::Param => reasons::SAME_PARAM,
        _ => reasons::SAME_EDGE,
    }
}

/// As `overwrite_conflict`, after a successful re-run: the first unmirrored record that changed
/// a clip ours puts back whole (a restore): `SAME_EDGE` when it touched the clip's end edge,
/// `SAME_PARAM` for any other touch (`start`, or the clip as a whole), creation or removal.
fn overwrite_conflict(
    ours: &Fp,
    own: &HashSet<String>,
    theirs: &[(String, Fp)],
) -> Option<(&'static str, String)> {
    let put_back: Vec<&String> = ours
        .overwrites
        .iter()
        .filter(|x| !own.contains(*x))
        .collect();
    theirs.iter().find_map(|(tx, f)| {
        let reason = put_back.iter().find_map(|x| {
            if f.touches.contains(&((*x).clone(), T::Edge)) {
                return Some(reasons::SAME_EDGE);
            }
            let other = f.touches.iter().any(|(y, _)| y == *x)
                || f.creates.contains(x)
                || f.removes.contains(x);
            other.then_some(reasons::SAME_PARAM)
        })?;
        Some((reason, tx.clone()))
    })
}

/// The entity rules: `REMOVED` (the last unmirrored record to remove or create a needed clip
/// removed it; the earliest such record) before `SAME_EDGE` / `SAME_PARAM` (the first
/// unmirrored record to touch the same end edge, or the same `start`, of a clip ours touches).
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
        if same.is_none() {
            same = ours
                .touches
                .iter()
                .find(|(x, t)| {
                    *t != T::Exists && !own.contains(x) && f.touches.contains(&(x.clone(), *t))
                })
                .map(|(_, t)| (reason_of(*t), i));
        }
    }
    let removed = needed.iter().filter_map(|(_, by)| *by).min();
    removed
        .map(|i| (reasons::REMOVED, i))
        .or(same)
        .map(|(r, i)| (r, theirs[i].0.clone()))
}

/// A predicted conflict: `(txId, opIndex, reason, againstTx)`.
pub type Pred = (String, usize, String, Option<String>);

/// What [`rerun`] predicts for one record.
pub struct Rerun {
    /// The ops as made, or the conflict the transcribed rules predict.
    pub out: Result<Vec<LOp>, Pred>,
    /// The independent oracle's verdict on the ops it reached.
    pub verdict: Verdict,
    /// The ops whose positions the model cannot follow ([`MBranch::risky`]).
    pub risky: Vec<usize>,
}

impl Rerun {
    /// [`takes`] for this record.
    pub fn takes(&self, c: &Pred) -> bool {
        takes(&self.risky, self.out.as_ref().err(), c)
    }
}

/// Whether the real conflict `c` of a record is one the model takes as it is: at an op whose
/// position it cannot follow (`risky`), with a position's reason, where the model reached no
/// earlier conflict (`want`; module docs, *Exclusions*).
pub fn takes(risky: &[usize], want: Option<&Pred>, c: &Pred) -> bool {
    risky.contains(&c.1)
        && super::lops::POSITION.contains(&c.2.as_str())
        && want.is_none_or(|w| w.1 >= c.1)
}

/// `ops` re-run one by one on `b` over `steps` (module docs). `dup`: the reason a re-created
/// clip that is there already gets (a copy's same origin: `ALREADY_PRESENT`; an undo's:
/// `ID_COLLISION`).
pub fn rerun(
    b: &mut MBranch,
    tx: &str,
    ops: &[LOp],
    steps: &[AStep],
    copy: bool,
    dup: &'static str,
) -> Rerun {
    let theirs = unmirrored(steps);
    let mut created = HashSet::new();
    let mut earlier: Vec<Fp> = Vec::new();
    let mut made = Vec::new();
    let mut verdict = None;
    let mut risky = Vec::new();
    let pred = |k: usize, reason: &str, against: Option<String>| -> Pred {
        (tx.to_string(), k, reason.to_string(), against)
    };
    for (k, op) in ops.iter().enumerate() {
        let r = b.risky(op, copy);
        if r {
            risky.push(k);
        }
        // Entities our own earlier ops created are never judged (spec §4.3): after this tx
        // removed and restored a clip, its trims act on the restored item.
        if verdict.is_none() && !created.contains(op.clip()) {
            verdict = b.verdict(op, copy).map(|class| (k, class, r));
        }
        let mut f = op.fp();
        // As `history::absorbs` marks it: a lift removal whose range the next op's strict trim
        // takes back into the left part puts the removed clip's content back. Here every such
        // pair is a split's (or its redo's) inverse: the model has no overwrite inserts, whose
        // undo has the same two ops and is no conflict.
        if let (
            LOp::RemoveLift { id, .. },
            Some(LOp::Trim {
                strict: true,
                delta,
                ..
            }),
        ) = (op, ops.get(k + 1))
            && *delta > 0
        {
            f.overwrites.push(id.clone());
        }
        if let Some((reason, against)) = entity_conflict(&f, &created, &theirs) {
            let out = Err(pred(k, reason, Some(against)));
            return Rerun {
                out,
                verdict,
                risky,
            };
        }
        match b.run(op, copy) {
            Ok(op) => {
                // A restore that re-ran would put its snapshot over a later edit of its clip.
                if let Some((reason, against)) = overwrite_conflict(&f, &created, &theirs) {
                    let out = Err(pred(k, reason, Some(against)));
                    return Rerun {
                        out,
                        verdict,
                        risky,
                    };
                }
                created.extend(f.creates.iter().cloned());
                made.push(op);
                earlier.push(f);
            }
            Err((code, ids)) => {
                // As `rerun_conflict`: the ids the op touches or its error names, and what an
                // earlier op of ours touched to create one of them.
                let mut named: HashSet<String> = f.touches.iter().map(|(x, _)| x.clone()).collect();
                // And what the op would create (as `rerun_conflict`).
                named.extend(f.creates.iter().cloned());
                named.extend(ids);
                let creators = named.clone();
                for e in &earlier {
                    if e.creates.iter().any(|c| creators.contains(c)) {
                        named.extend(e.touches.iter().map(|(x, _)| x.clone()));
                    }
                }
                let blamed: HashSet<&str> = theirs
                    .iter()
                    .filter(|(_, f)| {
                        let changed = f.touches.iter().map(|(y, _)| y);
                        let mut changed = changed.chain(&f.creates).chain(&f.removes);
                        changed.any(|y| named.contains(y))
                    })
                    .map(|(t, _)| t.as_str())
                    .collect();
                let against =
                    (blamed.len() == 1).then(|| blamed.iter().next().unwrap().to_string());
                let reason = if code == "DUPLICATE_ID" { dup } else { code };
                let out = Err(pred(k, reason, against));
                return Rerun {
                    out,
                    verdict,
                    risky,
                };
            }
        }
    }
    Rerun {
        out: Ok(made),
        verdict,
        risky,
    }
}

/// As the merge's `on_skipped`: a conflict blamed on a record in `skipped` depends on it.
pub fn on_skipped(mut c: Pred, skipped: &HashSet<String>) -> Pred {
    if c.3.as_ref().is_some_and(|t| skipped.contains(t)) {
        c.2 = reasons::DEPENDS_ON_SKIPPED.to_string();
    }
    c
}

/// A merge's steps from the side branch to main (both paths are whole: the side branch forks
/// at main's seq 0): the side records inverted, main's records with their mirrors, and which
/// main step pairs with which side record by origin root.
pub struct Paths {
    inv: Vec<AStep>,
    target: Vec<AStep>,
    cross: Vec<(usize, usize)>,
}

impl Paths {
    pub fn new(source: &[ARec], target: &[ARec]) -> Paths {
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

    pub fn steps(&self, i: usize, planned: &[(usize, AStep)]) -> Vec<AStep> {
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
