//! Walking the record tree (model and rules in [`super`]).
//!
//! 1. **The DB line.** Follow `Known::head` up to the branch start. If the folder does not
//!    hold `head`, wait (for `last_hash`) and decide nothing. Then extend the line with tail
//!    records the folder already holds in order (an export that renamed its file but crashed
//!    before marking): those go to `mark_exported`. The rest of the tail, if any, is a virtual
//!    line at the end, with `tail_epoch`.
//! 2. **Down the DB line**, every record with siblings is a competition between the DB's line
//!    and each sibling line:
//!    - the DB's line wins → the siblings are quarantined;
//!    - a sibling wins above or at `head` → `AppliedLineLost` fork (nothing can be un-applied);
//!    - a sibling wins inside the exported part of the tail → the tail diverged; continue on
//!      the winner;
//!    - a tie (or any two lines on sync storage) → fork.
//! 3. **At the end of the DB line**, the unexported tail competes with the folder's children
//!    the same way. Losing sets `local_tail_diverged`.
//! 4. **New records**: follow the unique child, or the epoch-rule winner, quarantining losers,
//!    until a leaf, a fork, or a record whose `txId` the DB already holds with other bytes
//!    (or that repeats on the chain): `TxIdReused` fork.
//!
//! Each step moves one record deeper in a finite tree, so the walk ends.

use std::collections::HashSet;

use super::graph::{Graph, ROOT};
use super::{ForkInfo, ForkReason, ForkSide, Known, QuarantineReason, Resolution};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Cand {
    Node(usize),
    Tail,
}

enum Decision {
    Win(Cand),
    Fork(ForkReason),
}

struct Walk<'g, 'a> {
    g: &'g Graph<'a>,
    known: &'g Known,
    /// Highest epoch in each node's subtree (the tail included where it attaches).
    sub: Vec<u64>,
    applied: Vec<usize>,
    applied_ids: HashSet<&'a str>,
    marked: Vec<usize>,
    lost: Vec<usize>,
    /// Tail txIds that are being set aside, so importing the same txId is not a reuse.
    dropped: HashSet<&'g str>,
    fork: Option<ForkInfo>,
    diverged: bool,
    waiting: Vec<String>,
}

pub(super) fn run(g: &Graph<'_>, known: &Known) -> Resolution {
    let mut w = Walk {
        g,
        known,
        sub: vec![],
        applied: vec![],
        applied_ids: HashSet::new(),
        marked: vec![],
        lost: vec![],
        dropped: HashSet::new(),
        fork: None,
        diverged: false,
        waiting: g.waiting.iter().cloned().collect(),
    };
    w.walk();
    w.finish()
}

impl<'g, 'a> Walk<'g, 'a> {
    fn parent(&self, n: usize) -> usize {
        self.g.nodes[n].parent.unwrap_or(ROOT)
    }

    fn walk(&mut self) {
        let g = self.g;
        // 1. The DB line.
        let mut line = vec![];
        if let Some(head) = &self.known.head {
            let Some(&n) = g.by_hash.get(head.as_str()).filter(|&&n| g.reachable[n]) else {
                if let Some(l) = &self.known.last_hash
                    && !g.seg_idx.contains_key(l.as_str())
                {
                    self.waiting.push(l.clone());
                }
                return;
            };
            let mut at = n;
            // Reachable nodes lead to ROOT; the bound only guards the loop.
            for _ in 0..=g.nodes.len() {
                line.push(at);
                match self.parent(at) {
                    ROOT => break,
                    p => at = p,
                }
            }
            line.reverse();
        }
        let n_known = line.len();
        let mut at = line.last().copied().unwrap_or(ROOT);
        for (_, hash) in &self.known.tail {
            match g.by_hash.get(hash.as_str()) {
                Some(&n) if g.reachable[n] && self.parent(n) == at => {
                    line.push(n);
                    at = n;
                }
                _ => break,
            }
        }
        let tail_left = self.known.tail.len() > line.len() - n_known;
        self.subtree_epochs(tail_left.then_some(at));

        // 2. Down the DB line.
        let mut parent = ROOT;
        for (i, &d) in line.iter().enumerate() {
            let kids = g.kids(parent);
            if kids.len() > 1 {
                let cands: Vec<Cand> = kids.iter().map(|&k| Cand::Node(k)).collect();
                match self.decide(&cands) {
                    Decision::Win(Cand::Node(win)) if win == d => self.lose_all(kids, d),
                    Decision::Win(Cand::Node(win)) if i >= n_known => {
                        self.marked.extend_from_slice(&line[n_known..i]);
                        self.diverge();
                        self.lose_all(kids, win);
                        self.forward(parent, Some(win));
                        return;
                    }
                    Decision::Win(_) => {
                        return self.set_fork(ForkReason::AppliedLineLost, parent, &cands);
                    }
                    Decision::Fork(reason) => {
                        if i >= n_known {
                            self.marked.extend_from_slice(&line[n_known..i]);
                        }
                        return self.set_fork(reason, parent, &cands);
                    }
                }
            }
            parent = d;
        }
        self.marked.extend_from_slice(&line[n_known..]);

        // 3. The unexported tail against the folder.
        let kids = g.kids(at);
        if tail_left && !kids.is_empty() {
            let mut cands: Vec<Cand> = kids.iter().map(|&k| Cand::Node(k)).collect();
            cands.push(Cand::Tail);
            match self.decide(&cands) {
                Decision::Win(Cand::Tail) => self.lose_all(kids, usize::MAX),
                Decision::Win(Cand::Node(win)) => {
                    self.diverge();
                    self.lose_all(kids, win);
                    self.forward(at, Some(win));
                }
                Decision::Fork(reason) => self.set_fork(reason, at, &cands),
            }
            return;
        }
        // 4. New records.
        self.forward(at, None);
    }

    /// Highest epoch per subtree: children before parents (reverse breadth-first order).
    fn subtree_epochs(&mut self, tail_at: Option<usize>) {
        let g = self.g;
        self.sub = g.nodes.iter().map(|n| n.epoch).collect();
        if let Some(a) = tail_at.filter(|&a| a != ROOT) {
            self.sub[a] = self.sub[a].max(self.known.tail_epoch);
        }
        for &n in g.order.iter().rev() {
            let p = self.parent(n);
            if p != ROOT {
                self.sub[p] = self.sub[p].max(self.sub[n]);
            }
        }
    }

    fn epoch(&self, c: Cand) -> u64 {
        match c {
            Cand::Node(n) => self.sub[n],
            Cand::Tail => self.known.tail_epoch,
        }
    }

    fn decide(&self, cands: &[Cand]) -> Decision {
        if cands.len() > 1 && self.known.kind.is_sync() {
            return Decision::Fork(ForkReason::SyncFolder);
        }
        let top = cands.iter().map(|&c| self.epoch(c)).max().unwrap_or(0);
        let mut best = cands.iter().filter(|&&c| self.epoch(c) == top);
        match (best.next(), best.next()) {
            (Some(&c), None) => Decision::Win(c),
            _ => Decision::Fork(ForkReason::EqualEpoch),
        }
    }

    /// Quarantines the subtrees of `kids` other than `keep`.
    fn lose_all(&mut self, kids: &[usize], keep: usize) {
        for &k in kids.iter().filter(|&&k| k != keep) {
            let mut stack = vec![k];
            while let Some(n) = stack.pop() {
                self.lost.push(n);
                stack.extend_from_slice(self.g.kids(n));
            }
        }
    }

    fn diverge(&mut self) {
        self.diverged = true;
        let marked: HashSet<&str> = self
            .marked
            .iter()
            .map(|&n| self.g.nodes[n].meta.tx_id.as_str())
            .collect();
        self.dropped = self
            .known
            .tail
            .iter()
            .map(|(id, _)| id.as_str())
            .filter(|id| !marked.contains(id))
            .collect();
    }

    /// Imports from `at` downwards, entering `enter` first when given.
    fn forward(&mut self, mut at: usize, enter: Option<usize>) {
        let g = self.g;
        if let Some(e) = enter {
            if !self.apply(at, e) {
                return;
            }
            at = e;
        }
        loop {
            let kids = g.kids(at);
            let next = match kids {
                [] => return,
                [only] => *only,
                _ => {
                    let cands: Vec<Cand> = kids.iter().map(|&k| Cand::Node(k)).collect();
                    match self.decide(&cands) {
                        Decision::Win(Cand::Node(win)) => {
                            self.lose_all(kids, win);
                            win
                        }
                        Decision::Win(Cand::Tail) => unreachable!("no tail among folder kids"),
                        Decision::Fork(reason) => return self.set_fork(reason, at, &cands),
                    }
                }
            };
            if !self.apply(at, next) {
                return;
            }
            at = next;
        }
    }

    fn apply(&mut self, parent: usize, n: usize) -> bool {
        let id = self.g.nodes[n].meta.tx_id.as_str();
        let held = self.known.known_tx.contains_key(id) && !self.dropped.contains(id);
        if held || !self.applied_ids.insert(id) {
            let reason = ForkReason::TxIdReused {
                tx_id: id.to_string(),
            };
            self.set_fork(reason, parent, &[Cand::Node(n)]);
            return false;
        }
        self.applied.push(n);
        true
    }

    fn set_fork(&mut self, reason: ForkReason, at: usize, cands: &[Cand]) {
        let mut sides: Vec<ForkSide> = cands
            .iter()
            .map(|&c| match c {
                Cand::Node(n) => ForkSide::Segment {
                    segment: self.seg_of(n).0.to_string(),
                    tx_hash: self.g.nodes[n].meta.tx_hash.clone(),
                    epoch: self.sub[n],
                },
                Cand::Tail => ForkSide::LocalTail {
                    epoch: self.known.tail_epoch,
                },
            })
            .collect();
        sides.sort_by(|a, b| match (a, b) {
            (ForkSide::Segment { tx_hash: x, .. }, ForkSide::Segment { tx_hash: y, .. }) => {
                x.cmp(y)
            }
            (ForkSide::LocalTail { .. }, _) => std::cmp::Ordering::Greater,
            (_, ForkSide::LocalTail { .. }) => std::cmp::Ordering::Less,
        });
        self.fork = Some(ForkInfo {
            reason,
            after: (at != ROOT).then(|| self.g.nodes[at].meta.tx_hash.clone()),
            sides,
        });
    }

    /// The canonical copy of a record: from a pack first, else the smallest segment hash.
    fn occ(&self, n: usize) -> (usize, usize) {
        let occ = &self.g.nodes[n].occ;
        occ.iter()
            .copied()
            .find(|&(s, _)| self.g.segs[s].header().pack)
            .unwrap_or(occ[0])
    }

    fn seg_of(&self, n: usize) -> (&'a str, usize) {
        let (s, r) = self.occ(n);
        (self.g.segs[s].hash(), r)
    }

    /// Consecutive records of one segment, as slices.
    fn slices(&self, occs: impl IntoIterator<Item = (usize, usize)>) -> Vec<super::SegRef> {
        let mut runs: Vec<(usize, usize, usize)> = vec![];
        for (s, r) in occs {
            match runs.last_mut() {
                Some((ls, _, end)) if *ls == s && *end == r => *end = r + 1,
                _ => runs.push((s, r, r + 1)),
            }
        }
        runs.into_iter()
            .map(|(s, a, b)| self.g.segs[s].slice(a..b))
            .collect()
    }

    fn finish(self) -> Resolution {
        let apply = self.slices(self.applied.iter().map(|&n| self.occ(n)));
        let mut lost: Vec<(usize, usize)> = self.lost.iter().map(|&n| self.occ(n)).collect();
        lost.sort_unstable();
        lost.dedup();
        let mut quarantine: Vec<_> = self
            .slices(lost)
            .into_iter()
            .map(|s| (s, QuarantineReason::StaleEpoch))
            .chain(
                self.g
                    .malformed
                    .iter()
                    .map(|(s, why)| (s.clone(), QuarantineReason::Malformed(why.clone()))),
            )
            .collect();
        quarantine.sort_by(|(a, _), (b, _)| {
            (a.hash(), a.range().start).cmp(&(b.hash(), b.range().start))
        });
        let mark_exported = self
            .marked
            .iter()
            .map(|&n| {
                let m = self.g.nodes[n].meta;
                (m.tx_id.clone(), self.seg_of(n).0.to_string())
            })
            .collect();
        let mut waiting = self.waiting;
        waiting.sort();
        waiting.dedup();
        Resolution {
            apply,
            mark_exported,
            quarantine,
            fork: self.fork,
            waiting,
            local_tail_diverged: self.diverged,
        }
    }
}
