//! Phase 2 (module docs of `compact`): after the grace period, check a manifest's packs and
//! snapshot against a fresh read of the folder, delete the segments they replace, then the
//! manifest. The reads, the "would a verdict change" resolutions and the fences run without
//! the store; the check against the database and every delete with it (module docs,
//! "Locking").

use std::collections::{BTreeSet, HashMap, HashSet};
use std::io;
use std::path::Path;

use super::manifest::{self, Manifest};
use super::planned::{Planned, Point};
use super::view::{self, Scanned, View};
use super::{Fence, Step, hit};
use crate::chain::{ForkSide, Known, Resolution, SegRef, resolve};
use crate::names::ManifestName;
use crate::snapfile::Snapshot;
use crate::storage_kind::StorageKind;
use crate::{Store, StoreAccess, StoreError, fsio};

/// What checking a manifest against the folder decided.
enum Checked {
    /// Delete these files (the replaced segments still present), in order.
    Delete(Vec<String>),
    Wait(String),
    Abandon(String),
}

/// What the check with the store hands the deletes: the view, the files to delete, and what
/// the verdicts are resolved with (the storage kind; this database's `Known`).
type ToDelete = (View, Vec<String>, StorageKind, Known);

/// Phase 2 for manifest `name` through `access`. `Step::Done`: the replaced files deleted and
/// the manifest with them. `deleted` counts the files deleted, whatever the step ends with
/// (one stopped or overtaken after some deletes did them). `None`: `access` lost the store.
pub(super) fn run<A: StoreAccess>(
    access: &mut A,
    planned: &Planned,
    name: &ManifestName,
    m: &Manifest,
    deleted: &mut u32,
) -> Result<Option<Step<()>>, StoreError> {
    let (vplan, dir) = with!(access, |s| -> Result<_, StoreError> {
        let dir = s.project_dir.join(".finisher").join("compaction");
        Ok((view::plan(s, &m.branch)?, dir))
    })?;
    // Without the store: the fence (a manifest written at an epoch above ours: someone holds
    // a newer lease), the manifest again, the branch afresh, the snapshot it names.
    if let Fence::Lost(why) = planned.check(m.epoch) {
        return Ok(Some(Step::Stopped(why)));
    }
    let path = dir.join(name.to_string());
    match manifest::read(&path) {
        manifest::Read::Ok(again) if again == *m => {}
        manifest::Read::Ok(_) => return Ok(Some(Step::Abandon("the manifest changed".into()))),
        manifest::Read::Gone => return Ok(Some(Step::Gone)),
        manifest::Read::Unreadable(why) => return Ok(Some(Step::Wait(why))),
    }
    let scanned = match vplan.scan() {
        Ok(s) => s,
        Err(why) => return Ok(Some(Step::Wait(why))),
    };
    let snap = m
        .snapshot
        .as_ref()
        .and_then(|n| scanned.snap(n))
        .map(|s| super::pack::read_snapshot(&s.path));
    planned.at(Point::CompactRead);
    // With it: the read checked against the database as it is now.
    let (v, files, kind, ours) = match with!(access, |s| check_now(s, planned, m, scanned, snap))? {
        Ok(checked) => checked,
        Err(step) => return Ok(Some(step)),
    };
    // Without it again (resolutions only): the deletes below run only while nothing changed
    // since the check, so `ours` is still this database's.
    if let Some(why) = verdict_changes(kind, &v, &files, ours) {
        return Ok(Some(Step::Abandon(why)));
    }
    for file in &files {
        planned.at(Point::CompactDelete);
        // The fence, right before every delete: the folder belongs to whoever holds the lease.
        if let Fence::Lost(why) = planned.check(m.epoch) {
            return Ok(Some(Step::Stopped(why)));
        }
        let path = v.seg_dir.join(file);
        match with!(access, |s| delete_now(
            s, planned, m.epoch, &v.seg_dir, &path, true
        ))? {
            Ok(true) => *deleted += 1,
            Ok(false) => {}
            Err(step) => return Ok(Some(step)),
        }
    }
    if *deleted > 0
        && let Err(e) = fsio::sync_dir(&v.seg_dir)
    {
        return Ok(Some(Step::Wait(format!(
            "cannot sync {}: {e}",
            v.seg_dir.display()
        ))));
    }
    with!(access, |s| hit(s, super::CompactFail::BeforeManifestDelete))?;
    if let Fence::Lost(why) = planned.check(m.epoch) {
        return Ok(Some(Step::Stopped(why)));
    }
    if let Err(step) = with!(access, |s| delete_now(
        s, planned, m.epoch, &dir, &path, false
    ))? {
        return Ok(Some(step));
    }
    let _ = fsio::sync_dir(&dir);
    Ok(Some(Step::Done(())))
}

/// With the store: unless overtaken, `scanned` checked against the database now
/// ([`Scanned::finish`], [`check`]), and this database's `Known` for the verdicts.
fn check_now(
    store: &mut Store,
    planned: &Planned,
    m: &Manifest,
    scanned: Scanned,
    snap: Option<Result<Snapshot, String>>,
) -> Result<Result<ToDelete, Step<()>>, StoreError> {
    if !planned.fresh(store) {
        return Ok(Err(Step::Overtaken));
    }
    let v = match scanned.finish(store)? {
        Ok(v) => v,
        Err(why) => return Ok(Err(Step::Wait(why))),
    };
    let files = match check(store, m, &v, snap)? {
        Checked::Delete(files) => files,
        Checked::Wait(why) => return Ok(Err(Step::Wait(why))),
        Checked::Abandon(why) => return Ok(Err(Step::Abandon(why))),
    };
    let kind = store.import.kind;
    let ours = crate::import::known_now(&store.conn, &m.branch, true, kind)?;
    Ok(Ok((v, files, kind, ours)))
}

/// Deletes `path` (in `dir`) with the store, only while nothing changed since the check
/// (`planned`: the same `io_gen` and lease) and the lease is still in force at `min_epoch`:
/// the delete then rests on the fresh read checked against the database as it is now (module
/// docs, "Locking"). `Ok(true)`: deleted; `Ok(false)`: absent already. `replaced`: a replaced
/// segment (the test hooks).
fn delete_now(
    store: &mut Store,
    planned: &Planned,
    min_epoch: u64,
    dir: &Path,
    path: &Path,
    replaced: bool,
) -> Result<Result<bool, Step<()>>, StoreError> {
    if !planned.fresh(store) {
        return Ok(Err(Step::Overtaken));
    }
    if let Fence::Lost(why) = planned.held(store, min_epoch) {
        return Ok(Err(Step::Stopped(why)));
    }
    if let Err(e) = crate::reserved::no_links(dir) {
        return Ok(Err(Step::Wait(e.to_string())));
    }
    let gone = match std::fs::remove_file(path) {
        Ok(()) => true,
        Err(e) if e.kind() == io::ErrorKind::NotFound => false,
        Err(e) => {
            return Ok(Err(Step::Wait(format!(
                "cannot delete {}: {e}",
                path.display()
            ))));
        }
    };
    if replaced {
        #[cfg(any(test, feature = "test-hooks"))]
        if let Some(f) = store.compact.on_delete.as_mut() {
            f(path);
        }
        hit(store, super::CompactFail::AfterDelete)?;
    }
    Ok(Ok(gone))
}

/// Checks everything before anything is deleted:
/// - every pack is present, a pack, not set aside, and every record it holds is on the
///   resolved chain and exported in the database;
/// - the snapshot (if named) verifies, lies on the chain and holds the database's state at
///   its seq;
/// - every replaced segment still present is not a pack, is listed in some pack's
///   `replaces`, is not set aside, and each of its records is on the chain, exported, and in
///   the packs byte for byte. A replaced segment that is absent was deleted already (an
///   interrupted phase 2) or never arrived here.
///
/// - no replaced segment is at an epoch above the pack that replaces it.
///
/// Missing files and a database behind the folder wait; anything that contradicts the
/// manifest abandons it (nothing is deleted, ever, for it).
fn check(
    store: &Store,
    m: &Manifest,
    v: &View,
    snap_read: Option<Result<Snapshot, String>>,
) -> Result<Checked, StoreError> {
    let mut records: HashMap<String, &[u8]> = HashMap::new();
    // Replaced hash → the highest epoch of a listed pack that replaces it.
    let mut replaces: HashMap<&str, u64> = HashMap::new();
    for p in &m.packs {
        if v.other_record.contains(p) {
            return Ok(Checked::Abandon(format!(
                "pack {p} names another branch record than the branch's"
            )));
        }
        let Some(s) = v.by_name.get(p) else {
            return Ok(Checked::Wait(format!("pack {p} is not in the folder")));
        };
        if !s.header().pack {
            return Ok(Checked::Abandon(format!("{p} is not a pack")));
        }
        if v.off_chain.contains(s.hash()) {
            return Ok(Checked::Abandon(format!("pack {p} is set aside")));
        }
        if !v.all_exported(s) {
            return Ok(
                if v.ahead(s)
                    && s.txs()
                        .iter()
                        .all(|t| v.chain.get(&t.seq) == Some(&t.tx_hash))
                {
                    Checked::Wait(format!("pack {p} is ahead of the database"))
                } else {
                    Checked::Abandon(format!("pack {p} holds records off the resolved chain"))
                },
            );
        }
        for (meta, bytes) in s.txs().iter().zip(s.records()) {
            records.insert(meta.tx_hash.clone(), bytes);
        }
        for r in &s.header().replaces {
            let e = replaces.entry(r.hash.as_str()).or_insert(0);
            *e = (*e).max(s.header().epoch);
        }
    }
    if let Some(snap) = &m.snapshot {
        let Some(s) = v.snaps.iter().find(|s| s.name.to_string() == *snap) else {
            return Ok(Checked::Wait(format!(
                "snapshot {snap} is not in the folder"
            )));
        };
        if s.header.project_id != store.project_id
            || v.chain.get(&s.header.seq) != Some(&s.header.tx_hash)
        {
            return Ok(Checked::Abandon(format!(
                "snapshot {snap} is not on the resolved chain"
            )));
        }
        // Read without the store with the branch (`run`).
        let read = snap_read.unwrap_or_else(|| Err(format!("{}: not read", s.path.display())));
        let matches = match read {
            Ok(snap) => super::pack::snapshot_matches(store, &m.branch, &s.path, &snap)?,
            Err(why) => Err(why),
        };
        if let Err(why) = matches {
            return Ok(Checked::Abandon(why));
        }
    }
    let mut delete = vec![];
    for r in &m.replaced {
        if v.other_record.contains(r) {
            return Ok(Checked::Abandon(format!(
                "{r} names another branch record than the branch's"
            )));
        }
        let Some(s) = v.by_name.get(r) else {
            continue;
        };
        if s.header().pack {
            return Ok(Checked::Abandon(format!("{r} is a pack")));
        }
        let Some(&pack_epoch) = replaces.get(s.hash()) else {
            return Ok(Checked::Abandon(format!("no pack replaces {r}")));
        };
        // Deleting it would lower the epoch of its records: a verdict could change.
        if s.header().epoch > pack_epoch {
            return Ok(Checked::Abandon(format!(
                "{r} is at epoch {}, above its pack's epoch {pack_epoch}",
                s.header().epoch
            )));
        }
        if v.off_chain.contains(s.hash()) {
            return Ok(Checked::Abandon(format!("{r} is set aside")));
        }
        for (meta, bytes) in s.txs().iter().zip(s.records()) {
            if records.get(&meta.tx_hash) != Some(&bytes) {
                return Ok(Checked::Abandon(format!(
                    "the packs do not hold seq {} of {r}",
                    meta.seq
                )));
            }
            if !v.exported(meta.seq, &meta.tx_hash) {
                return Ok(Checked::Abandon(format!(
                    "seq {} of {r} is not on the resolved chain",
                    meta.seq
                )));
            }
        }
        if s.txs().len() != s.segment().records.len() {
            return Ok(Checked::Abandon(format!("{r} does not parse")));
        }
        delete.push(r.clone());
    }
    Ok(Checked::Delete(delete))
}

/// A fork: its reason, the shared record, and each side as `(first record, epoch)`.
type ForkVerdict = (String, Option<String>, Vec<(Option<String>, u64)>);

/// The verdicts of a resolution that do not depend on which file holds a record: the chain,
/// what is marked exported, what is set aside and why, the fork (its sides by record), what
/// waits and what is pending.
#[derive(Debug, PartialEq)]
struct Verdicts {
    chain: Vec<(u64, String)>,
    marked: Vec<String>,
    aside: BTreeSet<(String, String)>,
    fork: Option<ForkVerdict>,
    waiting: BTreeSet<String>,
    pending: BTreeSet<String>,
    diverged: bool,
}

fn verdicts(res: &Resolution) -> Verdicts {
    let records = |sl: &SegRef| {
        sl.txs()
            .iter()
            .map(|t| t.tx_hash.clone())
            .collect::<Vec<_>>()
    };
    Verdicts {
        chain: res
            .apply
            .iter()
            .flat_map(|sl| sl.txs().iter().map(|t| (t.seq, t.tx_hash.clone())))
            .collect(),
        marked: res.mark_exported.iter().map(|(id, _)| id.clone()).collect(),
        aside: res
            .quarantine
            .iter()
            .flat_map(|(sl, why)| {
                records(sl)
                    .into_iter()
                    .map(move |h| (h, format!("{why:?}")))
            })
            .collect(),
        fork: res.fork.as_ref().map(|f| {
            let sides = f
                .sides
                .iter()
                .map(|s| match s {
                    ForkSide::Segment { tx_hash, epoch, .. } => (Some(tx_hash.clone()), *epoch),
                    ForkSide::LocalTail { epoch } => (None, *epoch),
                })
                .collect();
            (format!("{:?}", f.reason), f.after.clone(), sides)
        }),
        waiting: res.waiting.iter().cloned().collect(),
        pending: res.pending.iter().flat_map(records).collect(),
        diverged: res.local_tail_diverged,
    }
}

/// Why deleting `files` would change what a reader decides, if it would: the branch resolved
/// without them must give the same verdicts as with them, for a fresh reader
/// (`Known::for_branch` at the branch's fork point)
/// and for this database (`ours`). (A file that is malformed only because its named
/// predecessor is present could otherwise link through a pack once it is gone; a replaced
/// segment at a higher epoch than its pack would lower a line's epoch.) Resolutions only: no
/// store, no folder I/O.
fn verdict_changes(kind: StorageKind, v: &View, files: &[String], ours: Known) -> Option<String> {
    let gone: HashSet<&str> = files
        .iter()
        .filter_map(|f| v.by_name.get(f).map(|s| s.hash()))
        .collect();
    let after: Vec<SegRef> = v
        .segs
        .iter()
        .filter(|s| !gone.contains(s.hash()))
        .cloned()
        .collect();
    // These two suffice: the record graph is built from the files alone (no DB input), and
    // only epochs, malformed files or lying packs can change with the file set, all of which
    // show in the verdicts of a fresh reader; this DB's line and tail add the competitions
    // that only it sees.
    for (who, known) in [
        ("a fresh reader", Known::for_branch(kind, v.fork.clone())),
        ("this database", ours),
    ] {
        let before = if who == "a fresh reader" {
            verdicts(&v.res)
        } else {
            verdicts(&resolve(&v.segs, &known))
        };
        let now = verdicts(&resolve(&after, &known));
        if before != now {
            return Some(format!(
                "deleting the replaced segments would change what {who} resolves"
            ));
        }
    }
    None
}
