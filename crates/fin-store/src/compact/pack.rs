//! Phase 1 (module docs of `compact`): plan the packs of one branch, then write the snapshot,
//! the packs and the manifest, each fenced, written once and read back.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use rusqlite::params;

use super::manifest::{self, Manifest};
use super::view::{self, View, seg_name};
use super::{Fence, Step, fence, fsio_write, hit};
use crate::chain::SegRef;
use crate::db::to_i64;
use crate::names::{ManifestName, SegmentName, SnapName, hash12};
use crate::segment::{self, Header, Replaced};
use crate::snapfile::{self, SnapHeader};
use crate::{Store, StoreError};

/// One pack to write.
pub(super) struct PackPlan {
    pub(super) name: String,
    pub(super) bytes: Vec<u8>,
    pub(super) hash: String,
    pub(super) header: Header,
    /// The names of the segments it replaces.
    pub(super) replaced: Vec<String>,
}

/// The snapshot at the branch head in the folder.
enum SnapPlan {
    /// A verified snapshot at that seq and `txHash` is there already.
    Existing { name: String, path: PathBuf },
    New {
        name: String,
        bytes: Vec<u8>,
        hash: String,
    },
}

/// What phase 1 found to do.
pub(super) struct Plan {
    pub(super) packs: Vec<PackPlan>,
    snapshot: Option<(u64, SnapPlan)>,
}

/// What may be packed: every non-pack segment wholly on the resolved chain and exported in
/// the database (so never a stale, malformed, quarantined or pending one), that `protected`
/// (an abandoned compaction's segments) does not hold, that no malformed file names as its
/// predecessor, and whose predecessor is a present segment or one a present pack replaces
/// (the branch start for the first).
fn eligible<'v>(
    v: &'v View,
    protected: &HashSet<String>,
    protected_names: &HashSet<String>,
) -> Vec<&'v SegRef> {
    let mut out: Vec<&SegRef> = v
        .segs
        .iter()
        .filter(|s| {
            let h = s.header();
            let linked = match &h.prev_segment_hash {
                None => Some(h.first_seq) == v.chain_start,
                Some(q) => v.present.contains(q) || v.pack_replaced.contains(q),
            };
            !h.pack
                && linked
                && !v.off_chain.contains(s.hash())
                && !protected.contains(s.hash())
                && !v.malformed_prevs.contains(s.hash())
                && seg_name(s).is_some_and(|n| !protected_names.contains(&n))
                && v.all_exported(s)
        })
        .collect();
    out.sort_by(|a, b| (a.header().first_seq, a.hash()).cmp(&(b.header().first_seq, b.hash())));
    out
}

/// Runs of segments that follow one another by seq with non-decreasing epochs and name the
/// same `branch` record: a pack never mixes an epoch below one before it, so its label (the
/// highest it replaces, that of its last segment) changes no epoch-rule verdict, and never
/// spans two branch records (Decision 33; the view holds one record's segments already).
fn runs<'v>(elig: &[&'v SegRef]) -> Vec<Vec<&'v SegRef>> {
    let mut runs: Vec<Vec<&SegRef>> = vec![];
    let mut cur: Vec<&SegRef> = vec![];
    for &s in elig {
        let h = s.header();
        if let Some(last) = cur.last().map(|l| l.header()) {
            if h.first_seq <= last.last_seq {
                continue; // a duplicate of a range already covered: considered below
            }
            if h.first_seq == last.last_seq + 1
                && h.epoch >= last.epoch
                && h.branch_record == last.branch_record
            {
                cur.push(s);
                continue;
            }
            runs.push(std::mem::take(&mut cur));
        }
        cur.push(s);
    }
    if !cur.is_empty() {
        runs.push(cur);
    }
    runs
}

/// Plans the packs and the snapshot for `v` (`None`: nothing worth packing).
pub(super) fn plan(
    store: &Store,
    branch: &str,
    v: &View,
    protected: &HashSet<String>,
    protected_names: &HashSet<String>,
    lease_epoch: u64,
) -> Result<Option<Plan>, StoreError> {
    let elig = eligible(v, protected, protected_names);
    let genesis = crate::project_file::db_genesis_hash(&store.conn)?;
    let mut packs = vec![];
    for run in runs(&elig) {
        let mut i = 0;
        while i < run.len() {
            let mut take = vec![];
            let (mut n, mut body) = (0usize, 0usize);
            for &s in &run[i..] {
                let rn = s.segment().records.len();
                let rb: usize = s.records().map(|r| r.len() + 1).sum();
                // Room for the header's `replaces` list (one hash per replaced file).
                let reserve = segment::FRAME_RESERVE + 80 * (take.len() + 1);
                if !take.is_empty()
                    && (n + rn > segment::max_records() || body + rb + reserve > segment::MAX_BYTES)
                {
                    break;
                }
                take.push(s);
                n += rn;
                body += rb;
            }
            i += take.len().max(1);
            while take.len() >= 2 {
                let p = encode(store, branch, &genesis, &take, &elig)?;
                if p.bytes.len() <= segment::MAX_BYTES {
                    packs.push(p);
                    break;
                }
                take.pop();
                i -= 1;
            }
        }
    }
    if packs.is_empty() {
        return Ok(None);
    }
    let seq = v.exported_through;
    let snapshot = match v.chain.get(&seq) {
        Some(tx_hash) => {
            snapshot_plan(store, branch, v, seq, tx_hash, &genesis, lease_epoch)?.map(|p| (seq, p))
        }
        None => None,
    };
    Ok(Some(Plan { packs, snapshot }))
}

/// The pack of `primaries` (consecutive), replacing them and every other eligible segment
/// wholly inside their range whose epoch is not above the last one's.
fn encode(
    store: &Store,
    branch: &str,
    genesis: &str,
    primaries: &[&SegRef],
    elig: &[&SegRef],
) -> Result<PackPlan, StoreError> {
    let first = primaries[0].header();
    let last = primaries[primaries.len() - 1].header();
    let epoch = last.epoch;
    let mut replaced: Vec<&SegRef> = primaries.to_vec();
    for &s in elig {
        let h = s.header();
        if h.first_seq >= first.first_seq
            && h.last_seq <= last.last_seq
            && h.epoch <= epoch
            && h.branch_record == first.branch_record
            && !replaced.iter().any(|r| r.hash() == s.hash())
        {
            replaced.push(s);
        }
    }
    let mut replaces: Vec<Replaced> = replaced
        .iter()
        .map(|s| Replaced {
            hash: s.hash().to_string(),
            last_tx_hash: s
                .txs()
                .last()
                .map(|m| m.tx_hash.clone())
                .unwrap_or_default(),
        })
        .collect();
    replaces.sort();
    let lines: Vec<&[u8]> = primaries.iter().flat_map(|s| s.records()).collect();
    // The highest its records need: v1 while they are all edits.
    let (format_version, min_reader_version) = segment::version_for_branch(branch, &lines);
    let header = Header {
        format_version,
        min_reader_version,
        project_id: store.project_id.clone(),
        branch: branch.to_string(),
        writer_id: store.server_id.clone(),
        epoch,
        first_seq: first.first_seq,
        last_seq: last.last_seq,
        prev_segment_hash: first.prev_segment_hash.clone(),
        op_schema_versions: [
            primaries
                .iter()
                .map(|s| s.header().op_schema_versions[0])
                .min()
                .unwrap_or(0),
            primaries
                .iter()
                .map(|s| s.header().op_schema_versions[1])
                .max()
                .unwrap_or(0),
        ],
        // Deterministic, so a pack planned again after a crash is the same file.
        created_at: replaced
            .iter()
            .map(|s| s.header().created_at)
            .max()
            .unwrap_or(0),
        genesis_hash: genesis.to_string(),
        // A branch's pack names the same `branch` record as the segments it replaces.
        branch_record: first.branch_record.clone(),
        pack: true,
        replaces,
    };
    let (bytes, hash) = segment::encode(&header, &lines);
    let name = hash12(&hash)
        .and_then(|h| SegmentName::new(epoch, first.first_seq, last.last_seq, h))
        .ok_or_else(|| fin_core::CoreError::new("DB_ERROR", "a pack does not fit a segment name"))?
        .to_string();
    let mut names: Vec<(u64, String)> = replaced
        .iter()
        .filter_map(|s| Some((s.header().first_seq, seg_name(s)?)))
        .collect();
    names.sort();
    Ok(PackPlan {
        name,
        bytes,
        hash,
        header,
        replaced: names.into_iter().map(|(_, n)| n).collect(),
    })
}

fn snapshot_plan(
    store: &Store,
    branch: &str,
    v: &View,
    seq: u64,
    tx_hash: &str,
    genesis: &str,
    lease_epoch: u64,
) -> Result<Option<SnapPlan>, StoreError> {
    if let Some(s) = v.snaps.iter().find(|s| {
        s.header.seq == seq
            && s.header.tx_hash == tx_hash
            && s.header.project_id == store.project_id
            && s.header.genesis_hash == genesis
    }) {
        return Ok(Some(SnapPlan::Existing {
            name: s.name.to_string(),
            path: s.path.clone(),
        }));
    }
    let project = view::state_at(store, branch, seq)?;
    let header = SnapHeader {
        format_version: snapfile::FORMAT_VERSION,
        min_reader_version: snapfile::MIN_READER_VERSION,
        project_id: store.project_id.clone(),
        branch: branch.to_string(),
        seq,
        tx_hash: tx_hash.to_string(),
        model_schema_version: project.schema_version,
        writer_id: store.server_id.clone(),
        epoch: lease_epoch,
        genesis_hash: genesis.to_string(),
    };
    let (bytes, hash) = snapfile::encode(&header, &project);
    if bytes.len() as u64 > crate::scan::SNAPSHOT_CAP {
        return Ok(None);
    }
    Ok(hash12(&hash)
        .and_then(|h| SnapName::new(seq, h))
        .map(|name| SnapPlan::New {
            name: name.to_string(),
            bytes,
            hash,
        }))
}

/// Whether the snapshot at `path` verifies and holds the database's state at its seq.
pub(super) fn snapshot_matches(
    store: &Store,
    branch: &str,
    path: &Path,
) -> Result<Result<(), String>, StoreError> {
    let bytes = match crate::fsio::read_uncached(path) {
        Ok(b) => b,
        Err(e) => return Ok(Err(format!("{}: {e}", path.display()))),
    };
    let snap = match snapfile::decode(&bytes) {
        Ok(s) => s,
        Err(e) => return Ok(Err(format!("{}: {e}", path.display()))),
    };
    let ours = view::state_at(store, branch, snap.header.seq)?;
    let theirs = serde_json::from_slice(&snap.body)
        .ok()
        .and_then(|v| fin_core::upcast_snapshot(v).ok());
    let at = crate::db::hash_at(&store.conn, branch, snap.header.seq)?;
    if theirs.as_ref() != Some(&ours) || at.as_deref() != Some(snap.header.tx_hash.as_str()) {
        return Ok(Err(format!(
            "{} does not hold the state at seq {}",
            path.display(),
            snap.header.seq
        )));
    }
    Ok(Ok(()))
}

/// Writes `plan`: snapshot, packs (each recorded in `segments` once read back), manifest.
pub(super) fn write(
    store: &mut Store,
    branch: &str,
    plan: Plan,
    now_ms: i64,
) -> Result<Step<(ManifestName, Manifest)>, StoreError> {
    let dot = store.project_dir.join(".finisher");
    let mut lease_epoch = match fence(store, 0) {
        Fence::Held(e) => e,
        Fence::Lost(why) => return Ok(Step::Stopped(why)),
    };
    let snapshot = match plan.snapshot {
        None => None,
        Some((_, SnapPlan::Existing { name, path })) => {
            match snapshot_matches(store, branch, &path)? {
                Ok(()) => Some(name),
                Err(why) => return Ok(Step::Wait(why)),
            }
        }
        Some((_, SnapPlan::New { name, bytes, hash })) => {
            let dir = dot.join("snapshots").join(branch);
            let check = |b: &[u8]| snapfile::decode(b).is_ok_and(|s| s.hash == hash);
            if let Err(why) = fsio_write(&dir, &name, &bytes, &check) {
                return Ok(Step::Wait(why));
            }
            Some(name)
        }
    };
    hit(store, super::CompactFail::AfterSnapshot)?;
    for p in &plan.packs {
        lease_epoch = match fence(store, p.header.epoch) {
            Fence::Held(e) => e,
            Fence::Lost(why) => return Ok(Step::Stopped(why)),
        };
        let dir = dot.join("segments").join(branch);
        let check = |b: &[u8]| segment::decode(b).is_ok_and(|s| s.hash == p.hash);
        if let Err(why) = fsio_write(&dir, &p.name, &p.bytes, &check) {
            return Ok(Step::Wait(why));
        }
        let h = &p.header;
        store.conn.execute(
            "INSERT INTO segments (branch, hash, name, epoch, first_seq, last_seq, pack, replaces, state)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, 1, ?7, 'imported') ON CONFLICT(hash) DO NOTHING",
            params![
                branch,
                p.hash,
                p.name,
                to_i64(h.epoch)?,
                to_i64(h.first_seq)?,
                to_i64(h.last_seq)?,
                serde_json::to_string(&h.replaces)?,
            ],
        )?;
        hit(store, super::CompactFail::AfterPack)?;
    }
    let m = Manifest {
        magic: manifest::MAGIC.to_string(),
        format_version: manifest::FORMAT_VERSION,
        min_reader_version: manifest::FORMAT_VERSION,
        project_id: store.project_id.clone(),
        writer_id: store.server_id.clone(),
        epoch: lease_epoch,
        branch: branch.to_string(),
        replaced: plan.packs.iter().flat_map(|p| p.replaced.clone()).collect(),
        packs: plan.packs.iter().map(|p| p.name.clone()).collect(),
        snapshot,
        created_ms: now_ms,
        kind: store.storage_kind().to_string(),
    };
    if let Fence::Lost(why) = fence(store, 0) {
        return Ok(Step::Stopped(why));
    }
    let name = ManifestName::new();
    let bytes = m.encode();
    let check = |b: &[u8]| b == bytes.as_slice();
    if let Err(why) = fsio_write(&dot.join("compaction"), &name.to_string(), &bytes, &check) {
        return Ok(Step::Wait(why));
    }
    hit(store, super::CompactFail::AfterManifest)?;
    Ok(Step::Done((name, m)))
}
