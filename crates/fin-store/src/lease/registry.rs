//! What this process knows about its own leases, per (folder, server id).
//!
//! Two questions only this process can answer:
//! - **Is one of our own `Lease`s still heartbeating this folder?** Then another acquisition
//!   by the same server (a second `Store` on the same project) must see "held", not its own
//!   record to re-take.
//! - **Did this process write the record now in the folder, and has that lease stopped?**
//!   Then nobody else can have written since (the record would differ) and no thread of
//!   ours still writes it, so re-acquiring needs no wait for staleness: after `LEASE_LOST`
//!   (self-fence, the folder vanishing and returning) or after a `Store` closed. It still
//!   takes a fresh epoch through a claim, so a takeover by another server racing it is
//!   decided by the claim name.
//!
//! A record with our server id that this process did not write (a previous run, or another
//! process using the same server data) gets no shortcut: it is judged like anyone's.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use super::clock::lock;
use super::record::LeaseRecord;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(super) struct Key(PathBuf, String);

#[derive(Debug)]
struct Entry {
    epoch: u64,
    live: bool,
    last: LeaseRecord,
}

fn table() -> &'static Mutex<HashMap<Key, Entry>> {
    static TABLE: OnceLock<Mutex<HashMap<Key, Entry>>> = OnceLock::new();
    TABLE.get_or_init(Mutex::default)
}

/// The folder as one spelling: canonical while it exists, else absolute.
pub(super) fn key(dir: &Path, server_id: &str) -> Key {
    let path = std::fs::canonicalize(dir)
        .or_else(|_| std::path::absolute(dir))
        .unwrap_or_else(|_| dir.to_path_buf());
    Key(path, server_id.to_string())
}

/// A lease of ours was acquired with `record`.
pub(super) fn started(key: &Key, record: &LeaseRecord) {
    lock(table()).insert(
        key.clone(),
        Entry {
            epoch: record.epoch,
            live: true,
            last: record.clone(),
        },
    );
}

/// Our lease at `record.epoch` wrote `record`.
pub(super) fn wrote(key: &Key, record: &LeaseRecord) {
    if let Some(e) = lock(table()).get_mut(key)
        && e.epoch == record.epoch
    {
        e.last = record.clone();
    }
}

/// Our lease at `epoch` no longer writes (its heartbeat thread has ended).
pub(super) fn stopped(key: &Key, epoch: u64) {
    if let Some(e) = lock(table()).get_mut(key)
        && e.epoch == epoch
    {
        e.live = false;
    }
}

pub(super) enum Mine {
    /// One of our leases is heartbeating this folder.
    Live,
    /// `record` is exactly what this process last wrote, and nothing of ours writes it now.
    Leftover,
    /// Nothing known.
    Unknown,
}

pub(super) fn lookup(key: &Key, record: &LeaseRecord) -> Mine {
    match lock(table()).get(key) {
        Some(e) if e.live => Mine::Live,
        Some(e) if e.last == *record => Mine::Leftover,
        _ => Mine::Unknown,
    }
}
