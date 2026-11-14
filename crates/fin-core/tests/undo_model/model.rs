//! The model's records, stacks and planning of inverses (module docs in `mod.rs`).

use std::collections::{BTreeMap, HashSet};

use fin_core::Project;
use fin_core::rebase::reasons;

use super::branch::MBranch;
use super::lops::{LOp, Verdict};
use super::rules::{Pred, rerun, steps_from};
use super::{SIDE, to_frames};

#[derive(Debug, Clone, PartialEq)]
pub enum Kind {
    Edit,
    Undo(String),
    Redo(String),
    Merge(String),
    Pick,
}

/// A record as the model sees it: its kind, origin root and list operations.
#[derive(Debug, Clone)]
pub struct ARec {
    pub tx: String,
    pub seq: u64,
    pub kind: Kind,
    pub root: String,
    pub ops: Vec<LOp>,
}

#[derive(Debug, Clone)]
pub struct Unit {
    pub id: String,
    pub records: Vec<String>,
    pub last_seq: u64,
    /// The redo record a redo unit takes its inverse from.
    pub redo: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct Stacks {
    pub undo: Vec<Unit>,
    pub redo: Vec<Unit>,
}

/// An inverse planned on a branch: the branch after it, or the predicted conflict; the
/// independent verdict; the ops whose positions the model cannot follow.
pub struct Planned {
    pub out: Result<MBranch, Pred>,
    pub verdict: Verdict,
    pub risky: Vec<usize>,
}

#[derive(Debug, Clone, Default)]
pub struct Model {
    pub br: [MBranch; 2],
    pub stacks: BTreeMap<(usize, usize), Stacks>,
    /// Side records copied into main by a merge (Decision 22: they drop the side's stacks).
    pub merged_away: HashSet<String>,
}

impl Model {
    pub fn new(p: &Project) -> Model {
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
    pub fn stacks(&self, b: usize, a: usize) -> Stacks {
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

    pub fn raw(&mut self, b: usize, a: usize) -> &mut Stacks {
        self.stacks.entry((b, a)).or_default()
    }

    pub fn push(&mut self, b: usize, rec: ARec) {
        self.br[b].recs.push(rec);
    }

    /// A new unit of `a` on `b` (an edit, a pick, a merge group).
    pub fn push_unit(&mut self, b: usize, a: usize, id: &str, records: Vec<String>, seq: u64) {
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
    pub fn inverse_of(&self, b: usize, unit: &Unit) -> Vec<LOp> {
        let sources = match &unit.redo {
            Some(r) => vec![r.clone()],
            None => unit.records.clone(),
        };
        sources
            .iter()
            .rev()
            .flat_map(|t| self.br[b].rec(t).ops.iter().rev().flat_map(LOp::inverse))
            .collect()
    }

    fn after(&self, b: usize, seq: u64) -> Vec<&ARec> {
        self.br[b].recs.iter().filter(|r| r.seq > seq).collect()
    }

    /// Plans `ops` on `b` over its records after `at` (an undo or a redo), with the
    /// independent verdict.
    pub fn plan_inverse(&self, b: usize, tx: &str, at: u64, ops: &[LOp]) -> Planned {
        let steps = steps_from(&self.after(b, at));
        let mut next = self.br[b].clone();
        let r = rerun(&mut next, tx, ops, &steps, false, reasons::ID_COLLISION);
        let out = r.out.map(|made| {
            next.recs.push(ARec {
                tx: tx.to_string(),
                seq: 0,
                kind: Kind::Edit,
                root: tx.to_string(),
                ops: made,
            });
            next
        });
        Planned {
            out,
            verdict: r.verdict,
            risky: r.risky,
        }
    }

    /// The record `tx`, on whichever branch holds it.
    pub fn find(&self, tx: &str) -> Option<&ARec> {
        self.br.iter().flat_map(|b| &b.recs).find(|r| r.tx == tx)
    }

    /// The records of unit `u` on `b`: the record `u`, or the merge group `u`.
    pub fn unit_recs(&self, b: usize, u: &str) -> Vec<&ARec> {
        self.br[b]
            .recs
            .iter()
            .filter(|r| r.tx == u || r.kind == Kind::Merge(u.to_string()))
            .collect()
    }

    /// Whether branch `b` holds a record of `root` (its own records: every lineage here
    /// starts at main's seq 0, which holds no record).
    pub fn holds(&self, b: usize, root: &str) -> bool {
        self.br[b].recs.iter().any(|r| r.root == root)
    }

    /// Whether `r` (by its root record) is an undo or redo whose units hold a record that
    /// originates on `into`, directly or through another such record
    /// (`UNDOES_TARGET_RECORDS`).
    pub fn undoes_target(&self, into: usize, r: &ARec, depth: usize) -> bool {
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
