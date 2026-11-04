//! Chain resolution property tests against a reference model.
//!
//! The generator builds a random tree of segments (every segment starts where its parent
//! ends), random epochs (so stale lines get later-epoch descendants), a DB that has applied a
//! random path from the branch start, and a random unexported tail after it, partly exported
//! (a crash between rename and marking), with a random epoch, lease held or not, on local or
//! sync storage, with an optional gap. The reference model resolves that tree directly, by
//! segment, with the rules of spec §3.6 (line epoch = highest epoch in the line).

mod chain_support;

use std::collections::BTreeSet;

use chain_support::*;
use proptest::prelude::*;

#[derive(Debug, Clone)]
struct Gen {
    segs: Vec<(usize, u64, usize)>,
    db_walk: Vec<usize>,
    tail: Option<(usize, usize, u64)>,
    holds: bool,
    sync: bool,
    gap: Option<usize>,
    /// A later folder segment after the exported tail (or the head), at this epoch.
    after: Option<u64>,
}

fn scenarios() -> impl Strategy<Value = Gen> {
    (
        prop::collection::vec((0usize..64, 1u64..12, 1usize..3), 1..10),
        prop::collection::vec(0usize..4, 0..6),
        prop::option::of((1usize..4, 0usize..4, 1u64..12)),
        any::<bool>(),
        any::<bool>(),
        prop::option::of(0usize..10),
        prop::option::of(1u64..12),
    )
        .prop_map(|(segs, db_walk, tail, holds, sync, gap, after)| Gen {
            segs,
            db_walk,
            tail,
            holds,
            sync,
            gap,
            after,
        })
}

/// The generated folder and DB.
struct World {
    segs: Vec<SegRef>,
    parent: Vec<Option<usize>>,
    names: Vec<Vec<String>>,
    present: Vec<bool>,
    db_path: Vec<usize>,
    /// The exported part of the tail, a segment in `segs`.
    ex: Option<usize>,
    unexported: Vec<String>,
    known: Known,
}

impl World {
    fn input(&self) -> Vec<SegRef> {
        (0..self.segs.len())
            .filter(|&i| self.present[i])
            .map(|i| self.segs[i].clone())
            .collect()
    }
}

fn world(g: &Gen) -> World {
    let mut segs: Vec<SegRef> = vec![];
    let mut parent = vec![];
    let mut names = vec![];
    for (i, &(choice, epoch, len)) in g.segs.iter().enumerate() {
        // Mostly extend the newest segment; sometimes branch off anywhere (or the start).
        let p = if i == 0 {
            None
        } else if choice % 3 != 0 {
            Some(i - 1)
        } else {
            match (choice / 3) % (i + 1) {
                0 => None,
                k => Some(k - 1),
            }
        };
        let n: Vec<String> = (0..len).map(|j| format!("s{i}_{j}")).collect();
        let txs: Vec<&str> = n.iter().map(String::as_str).collect();
        segs.push(next(p.map(|p| &segs[p]), epoch, &txs));
        parent.push(p);
        names.push(n);
    }
    let kids = |parent: &Vec<Option<usize>>, n: Option<usize>| -> Vec<usize> {
        (0..parent.len()).filter(|&j| parent[j] == n).collect()
    };
    let mut db_path = vec![];
    let mut at = None;
    for &c in &g.db_walk {
        let k = kids(&parent, at);
        if k.is_empty() {
            break;
        }
        at = Some(k[c % k.len()]);
        db_path.push(at.unwrap());
    }
    let refs: Vec<&SegRef> = db_path.iter().map(|&i| &segs[i]).collect();
    let mut known = known(&refs, if g.sync { SYNC } else { LOCAL });
    let mut ex = None;
    let mut unexported = vec![];
    if let Some((m, exc, epoch)) = g.tail {
        let tn: Vec<String> = (0..m).map(|j| format!("t{j}")).collect();
        let split = exc % (m + 1);
        let mut prev = at.map(|i| segs[i].clone());
        if split > 0 {
            let txs: Vec<&str> = tn[..split].iter().map(String::as_str).collect();
            let s = next(prev.as_ref(), epoch, &txs);
            known = with_tail(known, &s, epoch);
            segs.push(s.clone());
            parent.push(at);
            names.push(tn[..split].to_vec());
            ex = Some(segs.len() - 1);
            prev = Some(s);
        }
        if split < m {
            let txs: Vec<&str> = tn[split..].iter().map(String::as_str).collect();
            known = with_tail(known, &next(prev.as_ref(), epoch, &txs), epoch);
            unexported = tn[split..].to_vec();
        }
        known.tail_epoch = epoch;
    }
    if let Some(epoch) = g.after {
        let p = ex.or(at);
        segs.push(next(p.map(|p| &segs[p]), epoch, &["late"]));
        parent.push(p);
        names.push(vec!["late".to_string()]);
    }
    known.holds_lease = g.holds;
    let mut present = vec![true; segs.len()];
    if let Some(gp) = g.gap {
        let gp = gp % segs.len();
        if !db_path.contains(&gp) && ex != Some(gp) {
            present[gp] = false;
        }
    }
    World {
        segs,
        parent,
        names,
        present,
        db_path,
        ex,
        unexported,
        known,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Outcome {
    applied: Vec<String>,
    marked: Vec<String>,
    quarantined: BTreeSet<String>,
    pending: BTreeSet<String>,
    waiting: BTreeSet<String>,
    diverged: bool,
    fork: Option<ForkReason>,
}

fn observe(r: &Resolution) -> Outcome {
    let names = |v: &[SegRef]| -> BTreeSet<String> {
        v.iter()
            .flat_map(|s| s.txs().iter().map(|m| m.tx_id.clone()))
            .collect()
    };
    Outcome {
        applied: applied(r),
        marked: r.mark_exported.iter().map(|(id, _)| id.clone()).collect(),
        quarantined: quarantined(r).into_iter().collect(),
        pending: names(&r.pending),
        waiting: r.waiting.iter().cloned().collect(),
        diverged: r.local_tail_diverged,
        fork: r.fork.as_ref().map(|f| f.reason.clone()),
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum C {
    Seg(usize),
    Tail,
}

/// The reference resolution, by segment.
struct Model<'w> {
    w: &'w World,
    sync: bool,
    tail_at: Option<Option<usize>>,
    out: Outcome,
    done: BTreeSet<usize>,
}

impl Model<'_> {
    fn kids(&self, n: Option<usize>) -> Vec<usize> {
        (0..self.w.segs.len())
            .filter(|&j| self.w.present[j] && self.w.parent[j] == n)
            .collect()
    }

    fn sub(&self, j: usize) -> u64 {
        let mut e = self.w.segs[j].header().epoch;
        if self.tail_at == Some(Some(j)) {
            e = e.max(self.w.known.tail_epoch);
        }
        self.kids(Some(j))
            .into_iter()
            .map(|k| self.sub(k))
            .fold(e, u64::max)
    }

    fn epoch(&self, c: C) -> u64 {
        match c {
            C::Seg(j) => self.sub(j),
            C::Tail => self.w.known.tail_epoch,
        }
    }

    fn decide(&self, cands: &[C]) -> Result<C, ForkReason> {
        if cands.len() > 1 && self.sync {
            return Err(ForkReason::SyncFolder);
        }
        let top = cands.iter().map(|&c| self.epoch(c)).max().unwrap();
        let best: Vec<C> = cands
            .iter()
            .copied()
            .filter(|&c| self.epoch(c) == top)
            .collect();
        if best.len() == 1 {
            Ok(best[0])
        } else {
            Err(ForkReason::EqualEpoch)
        }
    }

    fn lose(&mut self, j: usize) {
        self.done.insert(j);
        self.out.quarantined.extend(self.w.names[j].iter().cloned());
        for k in self.kids(Some(j)) {
            self.lose(k);
        }
    }

    fn lose_except(&mut self, kids: &[usize], keep: Option<usize>) {
        for &k in kids {
            if Some(k) != keep {
                self.lose(k);
            }
        }
    }

    fn apply(&mut self, j: usize) {
        self.done.insert(j);
        self.out.applied.extend(self.w.names[j].iter().cloned());
    }

    fn forward(&mut self, mut at: Option<usize>) {
        loop {
            let kids = self.kids(at);
            let next = match kids.len() {
                0 => return,
                1 => kids[0],
                _ => {
                    let cands: Vec<C> = kids.iter().map(|&k| C::Seg(k)).collect();
                    match self.decide(&cands) {
                        Ok(C::Seg(w)) => {
                            self.lose_except(&kids, Some(w));
                            w
                        }
                        Ok(C::Tail) => unreachable!(),
                        Err(f) => return self.out.fork = Some(f),
                    }
                }
            };
            self.apply(next);
            at = Some(next);
        }
    }

    fn run(&mut self) {
        let w = self.w;
        let mut line = w.db_path.clone();
        let n_known = line.len();
        line.extend(w.ex);
        let attach = line.last().copied();
        let tail_left = !w.unexported.is_empty();
        if tail_left && w.known.holds_lease {
            self.tail_at = Some(attach);
        }
        for &j in &w.db_path {
            self.done.insert(j);
        }
        let mut parent = None;
        for (i, &d) in line.iter().enumerate() {
            let kids = self.kids(parent);
            if kids.len() > 1 {
                let cands: Vec<C> = kids.iter().map(|&k| C::Seg(k)).collect();
                match self.decide(&cands) {
                    Ok(C::Seg(win)) if win == d => self.lose_except(&kids, Some(d)),
                    Ok(C::Seg(win)) if i >= n_known => {
                        self.out.diverged = true;
                        self.lose_except(&kids, Some(win));
                        self.apply(win);
                        return self.forward(Some(win));
                    }
                    Ok(_) => return self.out.fork = Some(ForkReason::AppliedLineLost),
                    Err(f) => return self.out.fork = Some(f),
                }
            }
            if i >= n_known {
                self.done.insert(d);
                self.out.marked.extend(w.names[d].iter().cloned());
            }
            parent = Some(d);
        }
        let kids = self.kids(attach);
        if tail_left && !kids.is_empty() {
            let mut cands: Vec<C> = kids.iter().map(|&k| C::Seg(k)).collect();
            if w.known.holds_lease {
                cands.push(C::Tail);
            } else if self.sync {
                // Spec §3.5: the lease is advisory on sync storage; the user chooses.
                return self.out.fork = Some(ForkReason::SyncFolder);
            } else {
                // Spec §3.6 step 7: without the lease the tail is set aside.
                self.out.diverged = true;
            }
            let pick = if cands.len() == 1 {
                Ok(cands[0])
            } else {
                self.decide(&cands)
            };
            match pick {
                Ok(C::Tail) => self.lose_except(&kids, None),
                Ok(C::Seg(win)) => {
                    self.out.diverged = true;
                    self.lose_except(&kids, Some(win));
                    self.apply(win);
                    self.forward(Some(win));
                }
                Err(f) => self.out.fork = Some(f),
            }
            return;
        }
        self.forward(attach);
    }
}

fn model(w: &World) -> Model<'_> {
    let mut m = Model {
        w,
        sync: w.known.kind.is_sync(),
        tail_at: None,
        out: Outcome {
            applied: vec![],
            marked: vec![],
            quarantined: BTreeSet::new(),
            pending: BTreeSet::new(),
            waiting: BTreeSet::new(),
            diverged: false,
            fork: None,
        },
        done: BTreeSet::new(),
    };
    if !w.unexported.is_empty() && w.known.holds_lease {
        m.tail_at = Some(w.ex.or(w.db_path.last().copied()));
    }
    m
}

/// The highest epoch in segment `j`'s subtree, as the reference sees it.
fn subtree_epoch(w: &World, j: usize) -> u64 {
    model(w).sub(j)
}

fn reference(w: &World) -> Outcome {
    let mut m = model(w);
    m.run();
    for j in 0..w.segs.len() {
        if !w.present[j] {
            continue;
        }
        if let Some(p) = w.parent[j].filter(|&p| !w.present[p]) {
            m.out.waiting.insert(w.segs[p].hash().to_string());
        }
        if !m.done.contains(&j) {
            m.out.pending.extend(w.names[j].iter().cloned());
        }
    }
    m.out
}

/// A deterministic shuffle (splitmix64).
fn shuffle<T>(v: &mut [T], mut seed: u64) {
    for i in (1..v.len()).rev() {
        seed = seed.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = seed;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^= z >> 31;
        v.swap(i, (z % (i as u64 + 1)) as usize);
    }
}

proptest! {
    /// The resolution matches the reference: in particular no record of a line whose epoch
    /// is lower than a competing line's is ever applied, and with the lease a stale write
    /// never beats the unexported tail.
    #[test]
    fn prop_never_applies_lower_epoch_branch_after_higher(g in scenarios(), seed in any::<u64>()) {
        let w = world(&g);
        let mut input = w.input();
        shuffle(&mut input, seed);
        let r = resolve(&input, &w.known);
        prop_assert_eq!(observe(&r), reference(&w));
    }

    #[test]
    fn prop_resolution_is_order_independent(
        g in scenarios(),
        dup in prop::option::of(0usize..16),
        seed in any::<u64>(),
    ) {
        let w = world(&g);
        let mut input = w.input();
        let base = resolve(&input, &w.known);
        if let (Some(d), false) = (dup, input.is_empty()) {
            let d = d % input.len();
            input.push(input[d].clone());
        }
        shuffle(&mut input, seed);
        prop_assert_eq!(resolve(&input, &w.known), base.clone());
        input.reverse();
        prop_assert_eq!(resolve(&input, &w.known), base);
    }

    /// Packing any run of consecutive present segments (on the resolved line at the holder's
    /// epoch, elsewhere at the run's lowest epoch) changes nothing, in or after the grace
    /// window.
    #[test]
    fn prop_packing_preserves_resolution(
        g in scenarios(),
        start in 0usize..16,
        len in 1usize..5,
        grace in any::<bool>(),
        seed in any::<u64>(),
    ) {
        let w = world(&g);
        let before = resolve(&w.input(), &w.known);
        let want = observe(&before);

        let mut run = vec![start % w.segs.len()];
        if !w.present[run[0]] {
            return Ok(());
        }
        while run.len() < len {
            match w.parent[run[0]] {
                Some(p) if w.present[p] => run.insert(0, p),
                _ => break,
            }
        }
        let settled: BTreeSet<&String> = want.applied.iter().chain(&want.marked).collect();
        let db_line: BTreeSet<&String> = w.db_path.iter().flat_map(|&j| &w.names[j]).collect();
        let line_ok = want.fork.is_none();
        let on_line = run.iter().all(|&j| {
            w.names[j].iter().all(|n| settled.contains(n) || (line_ok && db_line.contains(n)))
        });
        // After the grace window a pack is the only holder of its records, so its epoch
        // must not lower any of them. On the resolved line a real compactor writes the
        // holder's epoch (only winners rise). Elsewhere use the run's highest epoch, and only
        // when that leaves every subtree's highest epoch unchanged.
        let run_max = run.iter().map(|&j| w.segs[j].header().epoch).max().unwrap();
        let epoch = if on_line {
            w.segs.iter().map(|s| s.header().epoch).chain([w.known.tail_epoch]).max().unwrap()
        } else if run.iter().all(|&j| subtree_epoch(&w, j) >= run_max) {
            run_max
        } else {
            return Ok(());
        };
        let refs: Vec<&SegRef> = run.iter().map(|&j| &w.segs[j]).collect();
        let packed = pack(epoch, &refs);
        let mut input: Vec<SegRef> = (0..w.segs.len())
            .filter(|&j| w.present[j] && (grace || !run.contains(&j)))
            .map(|j| w.segs[j].clone())
            .collect();
        input.push(packed);
        shuffle(&mut input, seed);
        let after = resolve(&input, &w.known);
        prop_assert_eq!(observe(&after), want);
    }
}

#[test]
fn reference_model_sees_every_category() {
    // Guards the generator: across many seeds, each outcome kind occurs.
    use proptest::strategy::ValueTree;
    use proptest::test_runner::TestRunner;
    let mut runner = TestRunner::deterministic();
    let (mut stale, mut fork, mut diverged, mut marked, mut waiting, mut applied) =
        (0, 0, 0, 0, 0, 0);
    for _ in 0..512 {
        let g = scenarios().new_tree(&mut runner).unwrap().current();
        let o = reference(&world(&g));
        stale += usize::from(!o.quarantined.is_empty());
        fork += usize::from(o.fork.is_some());
        diverged += usize::from(o.diverged);
        marked += usize::from(!o.marked.is_empty());
        waiting += usize::from(!o.waiting.is_empty());
        applied += usize::from(!o.applied.is_empty());
    }
    for (name, n) in [
        ("stale", stale),
        ("fork", fork),
        ("diverged", diverged),
        ("marked", marked),
        ("waiting", waiting),
        ("applied", applied),
    ] {
        assert!(n >= 10, "{name} occurred only {n} times in 512 scenarios");
    }
}
