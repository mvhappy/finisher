//! A fresh view of one branch, as both compaction phases need it: an uncached scan (every file
//! read and verified again), the chain a fresh reader resolves from it, and the database
//! checked against that chain. A 4B branch is the segments of its directory that name its
//! index record (`branchRecord`, plan 4B Decision 33), resolved from its fork point.
//!
//! In three parts, so the folder I/O runs without the store (`compact` module docs,
//! "Locking"): [`plan`] takes what the read needs from the database, [`ViewPlan::scan`]
//! lists, reads, verifies and resolves the folder, and [`Scanned::finish`] checks the result
//! against the database as it is then.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use fin_core::Project;
use rusqlite::params;

use crate::chain::{ForkPoint, Known, QuarantineReason, Resolution, SegRef, resolve};
use crate::db::to_u64;
use crate::names::{SegmentName, SnapName, hash12};
use crate::scan::{Entry, NotArrivedWhy};
use crate::snapfile::SnapHeader;
use crate::storage_kind::StorageKind;
use crate::{Store, StoreError};

/// A present snapshot of the branch.
pub(super) struct Snap {
    pub(super) name: SnapName,
    pub(super) header: SnapHeader,
    pub(super) path: PathBuf,
}

pub(super) struct View {
    /// Verified segments of the branch (this project's genesis), by file name.
    pub(super) by_name: HashMap<String, SegRef>,
    pub(super) segs: Vec<SegRef>,
    pub(super) snaps: Vec<Snap>,
    pub(super) seg_dir: PathBuf,
    /// The resolved chain: `seq → txHash`.
    pub(super) chain: HashMap<u64, String>,
    /// The first seq of the chain.
    pub(super) chain_start: Option<u64>,
    /// The database's exported rows: every one is on `chain`, through this seq (0: none).
    pub(super) exported_through: u64,
    /// Segments the resolution sets aside, wholly or in part (stale, malformed).
    pub(super) off_chain: HashSet<String>,
    /// Every hash some present pack that is not malformed lists in `replaces`.
    pub(super) pack_replaced: HashSet<String>,
    /// The predecessors malformed files name (`prevSegmentHash`). Deleting one could let the
    /// file link through a pack's `replaces`, so phase 1 never packs them.
    pub(super) malformed_prevs: HashSet<String>,
    pub(super) present: HashSet<String>,
    /// The fresh resolution of `segs` (`Known::for_branch` at the branch's fork point).
    pub(super) res: Resolution,
    /// Where the branch starts on its parent (`None`: `main`).
    pub(super) fork: Option<ForkPoint>,
    /// File names of the directory's segments that name another `branch` record than the
    /// index's (Decision 33): never packed, deleted or counted as the branch's.
    pub(super) other_record: HashSet<String>,
}

impl View {
    /// Whether record `(seq, hash)` is on the chain and exported in the database.
    pub(super) fn exported(&self, seq: u64, hash: &str) -> bool {
        seq <= self.exported_through && self.chain.get(&seq).is_some_and(|h| h == hash)
    }

    /// Whether every record of `s` is on the chain and exported.
    pub(super) fn all_exported(&self, s: &SegRef) -> bool {
        !s.txs().is_empty() && s.txs().iter().all(|m| self.exported(m.seq, &m.tx_hash))
    }

    /// Whether a record of `s` lies on the chain at a seq the database has not exported yet
    /// (the folder is ahead: wait), as opposed to off the chain (never).
    pub(super) fn ahead(&self, s: &SegRef) -> bool {
        s.txs()
            .iter()
            .any(|m| m.seq > self.exported_through && self.chain.get(&m.seq) == Some(&m.tx_hash))
    }
}

/// The name a verified segment has in the folder.
pub(super) fn seg_name(s: &SegRef) -> Option<String> {
    let h = s.header();
    hash12(s.hash())
        .and_then(|x| SegmentName::new(h.epoch, h.first_seq, h.last_seq, x))
        .map(|n| n.to_string())
}

/// What a fresh read of a branch needs from the database, taken with the store
/// ([`plan`]); the read itself ([`ViewPlan::scan`]) runs without it.
pub(super) struct ViewPlan {
    branch: String,
    project_dir: PathBuf,
    storage: StorageKind,
    kind: StorageKind,
    project_id: String,
    genesis: String,
    /// The `branch` record a 4B branch's segments name (Decision 33); `None` on `main`.
    record: Option<String>,
    fork: Option<ForkPoint>,
}

/// The parts of [`ViewPlan`] that come from the database, compared again by
/// [`Scanned::finish`].
fn db_inputs(
    store: &Store,
    branch: &str,
) -> Result<(String, Option<String>, Option<ForkPoint>), StoreError> {
    let genesis = crate::project_file::db_genesis_hash(&store.conn)?;
    let record = if branch == crate::MAIN {
        None
    } else {
        crate::branch::index::get(&store.conn, branch)?.and_then(|i| i.record)
    };
    let fork = crate::import::fork_point(&store.conn, branch)?;
    Ok((genesis, record, fork))
}

/// Plans a fresh read of `branch` (with the store).
pub(super) fn plan(store: &Store, branch: &str) -> Result<ViewPlan, StoreError> {
    let (genesis, record, fork) = db_inputs(store, branch)?;
    Ok(ViewPlan {
        branch: branch.to_string(),
        project_dir: store.project_dir.clone(),
        storage: store.storage_kind(),
        kind: store.import.kind,
        project_id: store.project_id.clone(),
        genesis,
        record,
        fork,
    })
}

/// A branch read afresh, not yet checked against the database ([`Scanned::finish`]).
pub(super) struct Scanned {
    branch: String,
    view: View,
    genesis: String,
    record: Option<String>,
}

impl ViewPlan {
    /// Reads the branch afresh (folder I/O, without the store). `Err(why)`: the folder is not
    /// settled enough to act on (a file still arriving, damaged or unreadable, a fork, a gap):
    /// wait.
    pub(super) fn scan(self) -> Result<Scanned, String> {
        let ViewPlan {
            branch,
            project_dir,
            storage,
            kind,
            project_id,
            genesis,
            record,
            fork,
        } = self;
        let dot = project_dir.join(".finisher");
        let seg_dir = dot.join("segments").join(&branch);
        let snap_dir = dot.join("snapshots").join(&branch);
        // Fresh (§3.9 Phase 1): a listing refresh where the client caches listings.
        let entries = crate::scan::Scanner::new()
            .scan_for(&project_dir, storage)
            .map_err(|e| format!("cannot list the project folder: {e}"))?;
        let mut segs = vec![];
        let mut snaps = vec![];
        for e in entries {
            match e {
                Entry::Segment { branch: b, seg, .. } if b == branch => {
                    if seg.header.genesis_hash != genesis || seg.header.project_id != project_id {
                        return Err(format!("segment {} is another project's", seg.hash));
                    }
                    segs.push(SegRef::new(seg));
                }
                Entry::Snapshot {
                    branch: b,
                    name,
                    header,
                    ..
                } if b == branch => snaps.push(Snap {
                    path: snap_dir.join(name.to_string()),
                    name,
                    header,
                }),
                Entry::NotArrived {
                    why: NotArrivedWhy::Temp,
                    ..
                } => {}
                Entry::NotArrived { path, .. }
                | Entry::Damaged { path, .. }
                | Entry::Unreadable { path, .. }
                | Entry::Newer { path, .. }
                | Entry::UnsupportedHash { path }
                    if path.starts_with(&seg_dir) =>
                {
                    return Err(format!("{} is not readable yet", path.display()));
                }
                Entry::Foreign {
                    path,
                    content: Some(_),
                    ..
                } if path.starts_with(&seg_dir) => {
                    return Err(format!("{} is a misnamed Finisher file", path.display()));
                }
                _ => {}
            }
        }
        // A 4B branch is the segments that name its `branch` record (plan 4B Decision 33):
        // the others (another server's branch under the same id) are never packed or deleted.
        let mut other_record = HashSet::new();
        if branch != crate::MAIN {
            let (own, others): (Vec<SegRef>, Vec<SegRef>) = segs
                .into_iter()
                .partition(|s| record.is_some() && s.header().branch_record == record);
            other_record = others.iter().filter_map(seg_name).collect();
            segs = own;
        }
        segs.sort_by(|a, b| a.hash().cmp(b.hash()));
        // A branch resolves from its fork point, as a fresh follower that knows the index would.
        let res: Resolution = resolve(&segs, &Known::for_branch(kind, fork.clone()));
        if let Some(f) = &res.fork {
            return Err(format!("the branch has forked ({:?})", f.reason));
        }
        if !res.waiting.is_empty() || !res.pending.is_empty() {
            return Err("segments are waiting for a predecessor".into());
        }
        let mut chain = HashMap::new();
        let mut chain_start = None;
        for sl in &res.apply {
            for m in sl.txs() {
                chain_start.get_or_insert(m.seq);
                chain.insert(m.seq, m.tx_hash.clone());
            }
        }
        let off_chain = res
            .quarantine
            .iter()
            .map(|(s, _)| s.hash().to_string())
            .collect::<HashSet<_>>();
        // A pack that does not hold what it says it replaces stands in for nothing; one that
        // is only partly on the chain (a lost line after the fork inside it) still does.
        let malformed: HashSet<&str> = res
            .quarantine
            .iter()
            .filter(|(_, why)| matches!(why, QuarantineReason::Malformed(_)))
            .map(|(s, _)| s.hash())
            .collect();
        let pack_replaced = segs
            .iter()
            .filter(|s| s.header().pack && !malformed.contains(s.hash()))
            .flat_map(|s| s.header().replaces.iter().map(|r| r.hash.clone()))
            .collect();
        let malformed_prevs = segs
            .iter()
            .filter(|s| malformed.contains(s.hash()))
            .filter_map(|s| s.header().prev_segment_hash.clone())
            .collect();
        let by_name = segs
            .iter()
            .filter_map(|s| Some((seg_name(s)?, s.clone())))
            .collect();
        let present = segs.iter().map(|s| s.hash().to_string()).collect();
        Ok(Scanned {
            branch,
            view: View {
                by_name,
                segs,
                snaps,
                seg_dir,
                chain,
                chain_start,
                exported_through: 0,
                off_chain,
                pack_replaced,
                malformed_prevs,
                present,
                res,
                fork,
                other_record,
            },
            genesis,
            record,
        })
    }
}

impl Scanned {
    /// The present snapshot of the branch named `name`.
    pub(super) fn snap(&self, name: &str) -> Option<&Snap> {
        self.view.snaps.iter().find(|s| s.name.to_string() == name)
    }

    /// Checks the read against the database as it is now (with the store): the inputs the
    /// read was planned from are unchanged, and the database's exported rows are exactly the
    /// chain's prefix. `Ok(Err(why))`: wait.
    pub(super) fn finish(self, store: &Store) -> Result<Result<View, String>, StoreError> {
        let (branch, mut v) = (self.branch, self.view);
        let (genesis, record, fork) = db_inputs(store, &branch)?;
        if genesis != self.genesis || record != self.record || fork != v.fork {
            return Ok(Err("the branch changed while it was read".into()));
        }
        let mut stmt = store.conn.prepare(
            "SELECT seq, tx_hash FROM txs WHERE branch = ?1 AND exported = 1 ORDER BY seq",
        )?;
        let rows = stmt.query_map(params![branch], |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?))
        })?;
        for row in rows {
            let (seq, hash) = row?;
            let seq = to_u64(seq)?;
            if v.chain.get(&seq) != Some(&hash) {
                return Ok(Err(format!(
                    "the database's exported seq {seq} is not on the folder's chain"
                )));
            }
            v.exported_through = seq;
        }
        Ok(Ok(v))
    }
}

/// The state of `branch` after `seq`, from the database.
pub(super) fn state_at(store: &Store, branch: &str, seq: u64) -> Result<Project, StoreError> {
    crate::branch::lineage::state_at(&store.conn, branch, seq)
}
