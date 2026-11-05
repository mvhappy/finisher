//! Phase 2 (module docs of `compact`): after the grace period, check a manifest's packs and
//! snapshot against a fresh read of the folder, delete the segments they replace, then the
//! manifest.

use std::collections::{HashMap, HashSet};
use std::io;

use super::manifest::Manifest;
use super::view::{self, View};
use super::{Fence, Step, fence, hit};
use crate::names::ManifestName;
use crate::{Store, StoreError, fsio};

/// What checking a manifest against the folder decided.
enum Checked {
    /// Delete these files (the replaced segments still present), in order.
    Delete(Vec<String>),
    Wait(String),
    Abandon(String),
}

/// Phase 2 for manifest `name`. `Step::Done(n)`: `n` files deleted and the manifest with them.
pub(super) fn run(
    store: &mut Store,
    name: &ManifestName,
    m: &Manifest,
) -> Result<Step<u32>, StoreError> {
    if let Fence::Lost(why) = fence(store, 0) {
        return Ok(Step::Stopped(why));
    }
    let v = match view::read(store, &m.branch)? {
        Ok(v) => v,
        Err(why) => return Ok(Step::Wait(why)),
    };
    let files = match check(store, m, &v)? {
        Checked::Delete(files) => files,
        Checked::Wait(why) => return Ok(Step::Wait(why)),
        Checked::Abandon(why) => return Ok(Step::Abandon(why)),
    };
    let mut deleted = 0;
    for file in &files {
        // The fence, right before every delete: the folder belongs to whoever holds the lease.
        if let Fence::Lost(why) = fence(store, 0) {
            return Ok(Step::Stopped(why));
        }
        let path = v.seg_dir.join(file);
        match std::fs::remove_file(&path) {
            Ok(()) => deleted += 1,
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Ok(Step::Wait(format!("cannot delete {}: {e}", path.display()))),
        }
        #[cfg(any(test, feature = "test-hooks"))]
        if let Some(f) = store.compact.on_delete.as_mut() {
            f(&path);
        }
        hit(store, super::CompactFail::AfterDelete)?;
    }
    if deleted > 0
        && let Err(e) = fsio::sync_dir(&v.seg_dir)
    {
        return Ok(Step::Wait(format!(
            "cannot sync {}: {e}",
            v.seg_dir.display()
        )));
    }
    hit(store, super::CompactFail::BeforeManifestDelete)?;
    if let Fence::Lost(why) = fence(store, 0) {
        return Ok(Step::Stopped(why));
    }
    let dir = store.project_dir.join(".finisher").join("compaction");
    let path = dir.join(name.to_string());
    match std::fs::remove_file(&path) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Ok(Step::Wait(format!("cannot delete {}: {e}", path.display()))),
    }
    let _ = fsio::sync_dir(&dir);
    Ok(Step::Done(deleted))
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
/// Missing files and a database behind the folder wait; anything that contradicts the
/// manifest abandons it (nothing is deleted, ever, for it).
fn check(store: &Store, m: &Manifest, v: &View) -> Result<Checked, StoreError> {
    let mut records: HashMap<String, &[u8]> = HashMap::new();
    let mut replaces: HashSet<&str> = HashSet::new();
    for p in &m.packs {
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
        replaces.extend(s.header().replaces.iter().map(String::as_str));
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
        if let Err(why) = super::pack::snapshot_matches(store, &m.branch, &s.path)? {
            return Ok(Checked::Abandon(why));
        }
    }
    let mut delete = vec![];
    for r in &m.replaced {
        let Some(s) = v.by_name.get(r) else {
            continue;
        };
        if s.header().pack {
            return Ok(Checked::Abandon(format!("{r} is a pack")));
        }
        if !replaces.contains(s.hash()) {
            return Ok(Checked::Abandon(format!("no pack replaces {r}")));
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
