//! `segment_checksums`, `chain_continuity` and `epoch_order` (`super` module docs): one fresh
//! scan of `.finisher/`, and per branch the chain a fresh reader resolves from it.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use rusqlite::params;
use serde_json::{Value, json};

use super::Subject;
use crate::chain::{Known, Resolution, SegRef, resolve};
use crate::db::{self, hash_bytes};
use crate::scan::{Entry, NotArrivedWhy};
use crate::segment::Segment;
use crate::{MAIN, StoreError};

/// A damaged file unchanged this long is corrupt, not arriving.
const SETTLED: Duration = Duration::from_secs(60);
const LIST: usize = 20;

pub(super) struct Folder {
    entries: Result<Vec<Entry>, String>,
    branches: Vec<BranchChain>,
}

struct BranchChain {
    branch: String,
    segs: Vec<SegRef>,
    res: Resolution,
    /// `(seq, txHash)` of the resolved chain, in order.
    chain: Vec<(u64, String)>,
    /// Lowest epoch of the present segments holding each record.
    epoch_of: HashMap<String, u64>,
}

fn rel(dir: &Path, p: &Path) -> String {
    p.strip_prefix(dir)
        .unwrap_or(p)
        .to_string_lossy()
        .into_owned()
}

impl Folder {
    pub(super) fn read(s: &Subject) -> Folder {
        let entries = crate::scan::scan(s.project_dir).map_err(|e| e.to_string());
        let mut by_branch: HashMap<String, Vec<SegRef>> = HashMap::new();
        if let Ok(es) = &entries {
            for e in es {
                if let Entry::Segment { branch, seg, .. } = e {
                    by_branch
                        .entry(branch.clone())
                        .or_default()
                        .push(SegRef::new(seg.clone()));
                }
            }
        }
        let mut names: BTreeSet<String> = by_branch.keys().cloned().collect();
        names.extend(db::branches(s.conn).unwrap_or_default());
        let branches = names
            .into_iter()
            .map(|branch| {
                let mut segs = by_branch.remove(&branch).unwrap_or_default();
                segs.sort_by(|a, b| a.hash().cmp(b.hash()));
                let res = resolve(&segs, &Known::empty(s.kind));
                let chain = res
                    .apply
                    .iter()
                    .flat_map(|sl| sl.txs().iter().map(|m| (m.seq, m.tx_hash.clone())))
                    .collect();
                let mut epoch_of: HashMap<String, u64> = HashMap::new();
                for seg in &segs {
                    for m in seg.txs() {
                        let e = epoch_of.entry(m.tx_hash.clone()).or_insert(u64::MAX);
                        *e = (*e).min(seg.header().epoch);
                    }
                }
                BranchChain {
                    branch,
                    segs,
                    res,
                    chain,
                    epoch_of,
                }
            })
            .collect();
        Folder { entries, branches }
    }

    fn listed(&self) -> Result<&[Entry], StoreError> {
        self.entries.as_deref().map_err(|e| {
            fin_core::CoreError::new("IO_ERROR", format!("cannot list the project folder: {e}"))
                .into()
        })
    }

    pub(super) fn checksums(&self, s: &Subject) -> Result<(bool, Value), StoreError> {
        let entries = self.listed()?;
        let dir = s.project_dir;
        let now = SystemTime::now();
        let settled = |m: Option<SystemTime>| {
            m.and_then(|m| now.duration_since(m).ok())
                .is_some_and(|age| age >= SETTLED)
        };
        let kind_of = |p: &Path| {
            if p.extension().is_some_and(|e| e == "fsnap") {
                "snapshot"
            } else {
                "segment"
            }
        };
        let (mut segments, mut snapshots) = (0u64, 0u64);
        let (mut corrupt, mut unsettled, mut unreadable, mut newer, mut placeholders) =
            (vec![], vec![], vec![], vec![], vec![]);
        let mut present: HashSet<String> = HashSet::new();
        let mut replaced: HashSet<String> = HashSet::new();
        for e in entries {
            match e {
                Entry::Segment { seg, .. } => {
                    segments += 1;
                    present.insert(seg.hash.clone());
                    replaced.extend(seg.header.replaces.iter().cloned());
                }
                Entry::Snapshot { .. } => snapshots += 1,
                Entry::Damaged {
                    path,
                    reason,
                    modified,
                    ..
                } => {
                    let v = json!({ "code": "SEGMENT_CORRUPT", "path": rel(dir, path),
                                    "kind": kind_of(path), "reason": reason });
                    if settled(*modified) {
                        corrupt.push(v);
                    } else {
                        unsettled.push(v);
                    }
                }
                Entry::NotArrived {
                    path,
                    why: NotArrivedWhy::Incomplete,
                } => {
                    let modified = std::fs::metadata(path).and_then(|m| m.modified()).ok();
                    let v = json!({ "code": "SEGMENT_CORRUPT", "path": rel(dir, path),
                                    "kind": kind_of(path), "reason": "incomplete" });
                    if settled(modified) {
                        corrupt.push(v);
                    } else {
                        unsettled.push(v);
                    }
                }
                Entry::NotArrived {
                    path,
                    why: NotArrivedWhy::Placeholder,
                } => placeholders.push(rel(dir, path)),
                Entry::Unreadable { path, error } => {
                    unreadable.push(json!({ "path": rel(dir, path), "error": error }))
                }
                Entry::Newer { path, .. } | Entry::UnsupportedHash { path } => {
                    newer.push(rel(dir, path))
                }
                _ => {}
            }
        }
        // Segments the database knows were in the folder.
        let mut missing = vec![];
        let mut stmt = s
            .conn
            .prepare("SELECT hash, name FROM segments WHERE state IN ('exported', 'imported')")?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
        for row in rows {
            let (hash, name) = row?;
            if !present.contains(&hash) && !replaced.contains(&hash) {
                missing.push(json!({ "hash": hash, "name": name }));
            }
        }
        let ok = corrupt.is_empty() && unreadable.is_empty() && missing.is_empty();
        Ok((
            ok,
            json!({
                "segments": segments, "snapshots": snapshots, "corrupt": corrupt,
                "unsettled": unsettled, "unreadable": unreadable, "newer": newer,
                "notHydrated": placeholders, "missing": missing,
            }),
        ))
    }

    pub(super) fn continuity(&self, s: &Subject) -> Result<(bool, Value), StoreError> {
        self.listed()?;
        let mut problems: Vec<Value> = vec![];
        let mut count = 0u64;
        let mut report = |p: Value| {
            count += 1;
            if problems.len() < LIST {
                problems.push(p);
            }
        };
        let mut branches = vec![];
        for b in &self.branches {
            let chain: HashMap<u64, &str> = b.chain.iter().map(|(s, h)| (*s, h.as_str())).collect();
            if let Some(f) = &b.res.fork {
                report(json!({ "branch": b.branch, "problem": "fork",
                               "reason": format!("{:?}", f.reason), "after": f.after }));
            }
            if !b.res.waiting.is_empty() {
                report(
                    json!({ "branch": b.branch, "problem": "waiting", "segments": b.res.waiting }),
                );
            }
            let mut stmt = s.conn.prepare(
                "SELECT seq, tx_id, tx_hash, parent_hash, exported, record_bytes FROM txs
                 WHERE branch = ?1 ORDER BY seq",
            )?;
            let mut rows = stmt.query(params![b.branch])?;
            let first: Option<i64> = s.conn.query_row(
                "SELECT MIN(seq) FROM snapshots WHERE branch = ?1",
                params![b.branch],
                |r| r.get(0),
            )?;
            let mut prev_seq = first.map_or(Ok(0), db::to_u64)?;
            let mut prev_hash: Option<String> = None;
            let mut checked_parent = b.branch == MAIN;
            let (mut n, mut exported_through, mut unexported_seen) = (0u64, 0u64, false);
            while let Some(row) = rows.next()? {
                n += 1;
                let seq = db::to_u64(row.get(0)?)?;
                let tx_id: String = row.get(1)?;
                let tx_hash: String = row.get(2)?;
                let parent: Option<String> = row.get(3)?;
                let exported: bool = row.get(4)?;
                let bytes: Vec<u8> = row.get(5)?;
                let at = |what: &str| json!({ "branch": b.branch, "seq": seq, "problem": what });
                if seq != prev_seq + 1 {
                    report(at("seq is not the next one"));
                }
                if checked_parent && parent != prev_hash {
                    report(at("parent_hash is not the previous row's tx_hash"));
                }
                checked_parent = true;
                if hash_bytes(&bytes) != tx_hash {
                    report(at("tx_hash is not the hash of record_bytes"));
                }
                match db::parse_record(Some(seq), &bytes) {
                    Ok(r) if r.seq == seq && r.tx_id == tx_id && r.parent_hash == parent => {}
                    Ok(_) => report(at("the record is not this row's (seq, txId or parentHash)")),
                    Err(e) => report(json!({ "branch": b.branch, "seq": seq,
                                             "problem": "the record does not read", "error": e })),
                }
                if exported {
                    if unexported_seen {
                        report(at("an exported row follows an unexported one"));
                    }
                    exported_through = seq;
                    if chain.get(&seq) != Some(&tx_hash.as_str()) {
                        report(at("an exported row is not on the folder's chain"));
                    }
                } else {
                    unexported_seen = true;
                }
                prev_seq = seq;
                prev_hash = Some(tx_hash);
            }
            let folder_through = b.chain.last().map_or(0, |c| c.0);
            branches.push(json!({
                "branch": b.branch, "rows": n, "exportedThrough": exported_through,
                "folderThrough": folder_through,
                "folderAhead": folder_through.saturating_sub(prev_seq),
            }));
        }
        Ok((
            count == 0,
            json!({ "branches": branches, "problems": problems, "problemCount": count }),
        ))
    }

    pub(super) fn epochs(&self, s: &Subject) -> Result<(bool, Value), StoreError> {
        self.listed()?;
        let mut violations = vec![];
        let mut pack_labels = vec![];
        let mut tail = vec![];
        let mut count = 0u64;
        let mut branches = vec![];
        for b in &self.branches {
            let mut prev: Option<(u64, u64)> = None;
            for (seq, hash) in &b.chain {
                let Some(&e) = b.epoch_of.get(hash) else {
                    continue;
                };
                if let Some((pseq, pe)) = prev
                    && e < pe
                {
                    count += 1;
                    if violations.len() < LIST {
                        violations.push(json!({ "branch": b.branch, "seq": seq, "epoch": e,
                                                "previousSeq": pseq, "previousEpoch": pe }));
                    }
                }
                prev = Some((*seq, e));
            }
            let by_hash: HashMap<&str, &Arc<Segment>> =
                b.segs.iter().map(|s| (s.hash(), s.segment())).collect();
            for seg in b.segs.iter().filter(|s| s.header().pack) {
                let max = seg
                    .header()
                    .replaces
                    .iter()
                    .filter_map(|h| by_hash.get(h.as_str()))
                    .map(|s| s.header.epoch)
                    .max();
                if let Some(max) = max
                    && max != seg.header().epoch
                {
                    count += 1;
                    pack_labels.push(json!({ "branch": b.branch, "pack": seg.hash(),
                                             "epoch": seg.header().epoch, "replacedMax": max }));
                }
            }
            // The database's unexported rows (commit epochs).
            let exported_max: Option<i64> = s.conn.query_row(
                "SELECT MAX(epoch) FROM txs WHERE branch = ?1 AND exported = 1",
                params![b.branch],
                |r| r.get(0),
            )?;
            let mut floor = exported_max.unwrap_or(0);
            let mut stmt = s.conn.prepare(
                "SELECT seq, epoch FROM txs WHERE branch = ?1 AND exported = 0 ORDER BY seq",
            )?;
            let rows = stmt.query_map(params![b.branch], |r| {
                Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?))
            })?;
            for row in rows {
                let (seq, e) = row?;
                if e < floor {
                    count += 1;
                    tail.push(
                        json!({ "branch": b.branch, "seq": seq, "epoch": e, "floor": floor }),
                    );
                }
                floor = floor.max(e);
            }
            branches.push(
                json!({ "branch": b.branch, "chain": b.chain.len(), "segments": b.segs.len() }),
            );
        }
        Ok((
            count == 0,
            json!({ "branches": branches, "violations": violations, "packLabels": pack_labels,
                    "tail": tail }),
        ))
    }
}
