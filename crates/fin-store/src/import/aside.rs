//! Folder segments of a branch directory that its live branch does not take (plan 4B
//! Decisions 8 and 33).
//!
//! Every segment of a 4B branch names the `branch` record that made its branch
//! (`branchRecord`, the record's `txHash`). The worklist resolves a known branch from the
//! segments that name the index's record ([`crate::import`]'s pass); [`leftovers`] classifies
//! every other segment, after the worklist, by the record it names:
//! - the record is in `txs` but inert: it lost an id collision (`ID_COLLISION`) or the index
//!   refused it (`MALFORMED`); a record of another branch id is `MALFORMED` too;
//! - the record is a current quarantine row (`STALE_EPOCH`, `SUPERSEDED`, `DIVERGED_TAIL`,
//!   `MALFORMED`, `ID_COLLISION`): every record of the segment goes to quarantine with that
//!   row's reason (origin `folder`, detail `branch record <hash12> set aside`), and so,
//!   recursively, do the grandchildren those records make;
//! - otherwise the record is unknown here: the directory waits
//!   (`ImportReport::unknown_branches`, never read-only).
//!
//! Each pass looks up each named record once by its hash (primary keys); nothing is scanned.
//! Whether a quarantine row is current comes from this pass's verdicts (`current`, plus what
//! this classification takes), and for the reason that stays current on its own
//! (`DIVERGED_TAIL`) from the row.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use rusqlite::{OptionalExtension, params};

use super::ctx::Ctx;
use super::quarantine::{self, Quarantined};
use crate::StoreError;
use crate::chain::SegRef;
use crate::db::hash_bytes;
use crate::names::hash12;

/// The reasons that take a branch's segments along.
const REASONS: [&str; 5] = [
    quarantine::STALE_EPOCH,
    quarantine::SUPERSEDED,
    quarantine::DIVERGED_TAIL,
    quarantine::MALFORMED,
    quarantine::ID_COLLISION,
];

/// What became of the `branch` record a segment names.
enum Named {
    /// Not here yet: wait.
    Unknown,
    /// Set aside, for this reason.
    Aside(String),
}

/// The state of `record` (a `txHash`) for directory `dir`.
fn named(
    ctx: &mut Ctx,
    dir: &str,
    record: &str,
    current: &dyn Fn(&str) -> bool,
) -> Result<Named, StoreError> {
    if ctx.has_hash(record)? {
        // In the log, but not the record the index holds for `dir`: inert.
        let row: Option<(Option<String>, Option<String>)> = ctx
            .txn
            .query_row(
                "SELECT l.targets, l.ignored FROM txs t JOIN record_links l ON l.tx_id = t.tx_id
                 WHERE t.tx_hash = ?1 AND t.kind = 'branch'",
                params![record],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        let Some((targets, ignored)) = row else {
            return Ok(Named::Aside(quarantine::MALFORMED.into()));
        };
        let ids: Vec<String> = targets
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default();
        return Ok(match ignored {
            _ if ids != [dir] => Named::Aside(quarantine::MALFORMED.into()),
            Some(why) if why.starts_with(quarantine::ID_COLLISION) => {
                Named::Aside(quarantine::ID_COLLISION.into())
            }
            Some(_) => Named::Aside(quarantine::MALFORMED.into()),
            // Live but not indexed (the index does not hold `dir` yet): wait.
            None => Named::Unknown,
        });
    }
    let row: Option<String> = ctx
        .txn
        .prepare_cached("SELECT reason FROM quarantine WHERE tx_hash = ?1")?
        .query_row(params![record], |r| r.get(0))
        .optional()?;
    Ok(match row {
        Some(reason)
            if REASONS.contains(&reason.as_str())
                && (reason == quarantine::DIVERGED_TAIL || current(record)) =>
        {
            Named::Aside(reason)
        }
        _ => Named::Unknown,
    })
}

/// Quarantines every record of `segs` the DB does not hold as `reason`, for the set-aside
/// branch record `record`; returns their `txHash`es.
fn take(
    ctx: &mut Ctx,
    branch: &str,
    segs: &[SegRef],
    record: &str,
    reason: &str,
) -> Result<BTreeSet<String>, StoreError> {
    let detail = format!(
        "branch record {} set aside",
        hash12(record).unwrap_or(record)
    );
    let mut out = BTreeSet::new();
    for s in segs {
        for bytes in s.records() {
            let tx_hash = hash_bytes(bytes);
            if ctx.has_hash(&tx_hash)? || !out.insert(tx_hash.clone()) {
                continue;
            }
            quarantine::upsert(
                ctx.txn,
                &Quarantined {
                    tx_hash: &tx_hash,
                    branch,
                    origin: "folder",
                    reason,
                    detail: &detail,
                    bytes,
                    now: ctx.now,
                },
            )?;
        }
    }
    Ok(out)
}

/// Classifies the segments no live branch takes (module docs): returns the records set
/// aside and the directories that wait. `dirs` are every folder segment by directory;
/// `current` holds this pass's verdicts (every branch's and the foreign files').
pub(super) fn leftovers(
    ctx: &mut Ctx,
    dirs: &HashMap<String, Vec<SegRef>>,
    current: &BTreeSet<String>,
) -> Result<(BTreeSet<String>, Vec<String>), StoreError> {
    // (directory, named record) → segments.
    let mut groups: BTreeMap<(String, String), Vec<SegRef>> = BTreeMap::new();
    for (dir, segs) in dirs {
        if dir == crate::MAIN {
            continue;
        }
        let live = crate::branch::index::get(ctx.txn, dir)?.and_then(|i| i.record);
        for s in segs {
            let Some(r) = &s.header().branch_record else {
                continue;
            };
            if live.as_ref() != Some(r) {
                groups
                    .entry((dir.clone(), r.clone()))
                    .or_default()
                    .push(s.clone());
            }
        }
    }
    let mut taken = BTreeSet::new();
    loop {
        let mut progress = false;
        let keys: Vec<(String, String)> = groups.keys().cloned().collect();
        for key in keys {
            let state = {
                let is_current = |h: &str| current.contains(h) || taken.contains(h);
                named(ctx, &key.0, &key.1, &is_current)?
            };
            if let Named::Aside(reason) = state {
                let segs = groups.remove(&key).unwrap_or_default();
                taken.extend(take(ctx, &key.0, &segs, &key.1, &reason)?);
                progress = true;
            }
        }
        if !progress {
            break;
        }
    }
    let unknown: BTreeSet<String> = groups.into_keys().map(|(d, _)| d).collect();
    Ok((taken, unknown.into_iter().collect()))
}
