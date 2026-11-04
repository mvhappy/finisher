//! Building the record tree from verified segments (model in [`super`]).
//!
//! 1. Sort by hash and drop repeated files.
//! 2. Per segment (local checks): records parse, seqs run `firstSeq..=lastSeq`, each record's
//!    `parentHash` is the previous record's `txHash`, no `txId` repeats, and a segment with a
//!    predecessor starts with a record that has a parent. Failing → malformed.
//! 3. Packs: a pack is honoured only if it holds every record of each present segment it
//!    lists in `replaces`; otherwise it is malformed. (A pack can therefore never stand for a
//!    segment it does not contain: not itself, not its successor, not a different line.)
//! 4. Records become nodes keyed by `txHash`.
//! 5. Per segment (link checks), its first record:
//!    - `prevSegmentHash` null: a child of the branch start, unless its `parentHash` names a
//!      record present here (malformed: it claims to start the branch but continues it);
//!    - `prevSegmentHash` neither a present segment nor in an honoured pack's `replaces`: the
//!      segment waits (a gap) and its first record is not linked through it;
//!    - otherwise the `parentHash` must be that predecessor's last record (or a record of the
//!      pack that replaces it) with `seq` one less; else malformed.
//!
//!    Later records link to the record before them. Malformed segments add no links and no
//!    epochs; their records are not linked through them.
//! 6. Reachability from the branch start (breadth first, each node once).

use std::collections::{BTreeSet, HashMap, HashSet, VecDeque};

use super::{RecMeta, SegRef};

/// The branch start, as a parent.
pub(super) const ROOT: usize = usize::MAX;

pub(super) struct Node<'a> {
    pub meta: &'a RecMeta,
    /// The highest epoch of the usable segments holding this record.
    pub epoch: u64,
    /// `(segment, record index)` in usable segments, in segment (hash) order.
    pub occ: Vec<(usize, usize)>,
    /// `Some(ROOT)`, `Some(node)` or `None` (not linked).
    pub parent: Option<usize>,
}

pub(super) struct Graph<'a> {
    /// Locally valid segments with honest `replaces`, in hash order.
    pub segs: Vec<&'a SegRef>,
    pub seg_idx: HashMap<&'a str, usize>,
    pub malformed: Vec<(SegRef, String)>,
    pub nodes: Vec<Node<'a>>,
    pub by_hash: HashMap<&'a str, usize>,
    children: Vec<Vec<usize>>,
    root_children: Vec<usize>,
    pub reachable: Vec<bool>,
    /// Reachable nodes, parents before children.
    pub order: Vec<usize>,
    /// Missing predecessors of segments whose first record is not reachable otherwise.
    pub waiting: BTreeSet<String>,
}

impl<'a> Graph<'a> {
    pub fn kids(&self, parent: usize) -> &[usize] {
        if parent == ROOT {
            &self.root_children
        } else {
            &self.children[parent]
        }
    }

    pub fn build(input: &'a [SegRef]) -> Graph<'a> {
        let mut sorted: Vec<&SegRef> = input.iter().collect();
        sorted.sort_by(|a, b| a.hash().cmp(b.hash()));
        sorted.dedup_by(|a, b| a.hash() == b.hash());

        let mut malformed = vec![];
        let mut valid = vec![];
        for s in sorted {
            match local_check(s) {
                Ok(()) => valid.push(s),
                Err(why) => malformed.push((s.whole(), why)),
            }
        }
        let segs = honest_packs(valid, &mut malformed);
        let seg_idx: HashMap<&str, usize> = segs
            .iter()
            .enumerate()
            .map(|(i, s)| (s.hash(), i))
            .collect();
        let mut alias: HashMap<&str, Vec<usize>> = HashMap::new();
        for (i, s) in segs.iter().enumerate() {
            for r in &s.header().replaces {
                alias.entry(r.as_str()).or_default().push(i);
            }
        }

        let mut nodes: Vec<Node> = vec![];
        let mut by_hash: HashMap<&str, usize> = HashMap::new();
        let mut all_occ: Vec<Vec<usize>> = vec![]; // node → segments, any usability
        for (si, s) in segs.iter().enumerate() {
            for m in s.all_txs() {
                let n = *by_hash.entry(m.tx_hash.as_str()).or_insert_with(|| {
                    nodes.push(Node {
                        meta: m,
                        epoch: 0,
                        occ: vec![],
                        parent: None,
                    });
                    all_occ.push(vec![]);
                    nodes.len() - 1
                });
                all_occ[n].push(si);
            }
        }

        // Link checks of each segment's first record.
        let mut usable = vec![true; segs.len()];
        let mut first_link: Vec<Option<usize>> = vec![None; segs.len()];
        let mut gaps: Vec<(usize, &str)> = vec![];
        for (si, s) in segs.iter().enumerate() {
            let m0 = &s.all_txs()[0];
            let parent = m0
                .parent_hash
                .as_deref()
                .and_then(|p| by_hash.get(p).copied());
            let verdict = match s.header().prev_segment_hash.as_deref() {
                None if parent.is_some() => {
                    Err("starts the branch but continues a record that is present".to_string())
                }
                None => Ok(Some(ROOT)),
                Some(q) if !seg_idx.contains_key(q) && !alias.contains_key(q) => {
                    gaps.push((si, q));
                    Ok(None)
                }
                Some(q) => {
                    let continues = parent.is_some_and(|x| {
                        let seq_ok = nodes[x].meta.seq.checked_add(1) == Some(m0.seq);
                        let place_ok = match seg_idx.get(q) {
                            Some(&qi) => {
                                segs[qi].all_txs().last().map(|l| &l.tx_hash)
                                    == m0.parent_hash.as_ref()
                            }
                            None => alias[q].iter().any(|pi| all_occ[x].contains(pi)),
                        };
                        seq_ok && place_ok
                    });
                    if continues {
                        Ok(parent)
                    } else {
                        Err(format!("does not continue its predecessor {}", short(q)))
                    }
                }
            };
            match verdict {
                Ok(link) => first_link[si] = link,
                Err(why) => {
                    usable[si] = false;
                    malformed.push((s.whole(), why));
                }
            }
        }

        for (si, s) in segs.iter().enumerate().filter(|(si, _)| usable[*si]) {
            let txs = s.all_txs();
            for (ri, m) in txs.iter().enumerate() {
                let n = by_hash[m.tx_hash.as_str()];
                let node = &mut nodes[n];
                node.occ.push((si, ri));
                node.epoch = node.epoch.max(s.header().epoch);
                let link = if ri == 0 {
                    first_link[si]
                } else {
                    Some(by_hash[txs[ri - 1].tx_hash.as_str()])
                };
                if let Some(p) = link
                    && p != n
                    && node.parent.is_none()
                {
                    node.parent = Some(p);
                }
            }
        }

        let mut children = vec![vec![]; nodes.len()];
        let mut root_children = vec![];
        for (n, node) in nodes.iter().enumerate() {
            match node.parent {
                Some(ROOT) => root_children.push(n),
                Some(p) => children[p].push(n),
                None => {}
            }
        }

        let mut reachable = vec![false; nodes.len()];
        let mut order = vec![];
        let mut queue: VecDeque<usize> = root_children.iter().copied().collect();
        for &n in &root_children {
            reachable[n] = true;
        }
        while let Some(n) = queue.pop_front() {
            order.push(n);
            for &c in &children[n] {
                if !reachable[c] {
                    reachable[c] = true;
                    queue.push_back(c);
                }
            }
        }

        let waiting = gaps
            .into_iter()
            .filter(|(si, _)| !reachable[by_hash[segs[*si].all_txs()[0].tx_hash.as_str()]])
            .map(|(_, q)| q.to_string())
            .collect();

        Graph {
            segs,
            seg_idx,
            malformed,
            nodes,
            by_hash,
            children,
            root_children,
            reachable,
            order,
            waiting,
        }
    }
}

fn local_check(s: &SegRef) -> Result<(), String> {
    if let Some(e) = s.parse_error() {
        return Err(e.to_string());
    }
    let h = s.header();
    let txs = s.all_txs();
    let Some(first) = txs.first() else {
        return Err("holds no records".into());
    };
    let mut ids = HashSet::new();
    for (i, m) in txs.iter().enumerate() {
        if Some(m.seq) != h.first_seq.checked_add(i as u64) {
            return Err(format!(
                "record {i} has seq {} in a segment from seq {}",
                m.seq, h.first_seq
            ));
        }
        if i > 0 && m.parent_hash.as_ref() != Some(&txs[i - 1].tx_hash) {
            return Err(format!("record {i} does not follow record {}", i - 1));
        }
        if !ids.insert(m.tx_id.as_str()) {
            return Err(format!("txId {} repeats", m.tx_id));
        }
    }
    if h.prev_segment_hash.is_some() && first.parent_hash.is_none() {
        return Err("has a predecessor but its first record has no parentHash".into());
    }
    Ok(())
}

/// Drops (as malformed) every pack that lists a present segment whose records it lacks.
fn honest_packs<'a>(
    valid: Vec<&'a SegRef>,
    malformed: &mut Vec<(SegRef, String)>,
) -> Vec<&'a SegRef> {
    let idx: HashMap<&str, usize> = valid
        .iter()
        .enumerate()
        .map(|(i, s)| (s.hash(), i))
        .collect();
    let mut keep = vec![];
    for (i, s) in valid.iter().enumerate() {
        let lie = s.header().pack.then(|| {
            let mine: HashSet<&str> = s.all_txs().iter().map(|m| m.tx_hash.as_str()).collect();
            s.header().replaces.iter().find(|r| {
                idx.get(r.as_str()).is_some_and(|&j| {
                    j == i
                        || !valid[j]
                            .all_txs()
                            .iter()
                            .all(|m| mine.contains(m.tx_hash.as_str()))
                })
            })
        });
        match lie.flatten() {
            Some(r) => malformed.push((
                s.whole(),
                format!("replaces {} without holding its records", short(r)),
            )),
            None => keep.push(*s),
        }
    }
    keep
}

/// `<hash12>` for messages.
pub(super) fn short(hash: &str) -> &str {
    hash.get(7..19).unwrap_or(hash)
}
