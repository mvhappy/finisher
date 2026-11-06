//! The `quarantine` table (spec §3.6 step 8, §3.7): records set aside, never applied.
//!
//! - One row per `txHash` (`UNIQUE`): a stale line is re-reported on every poll, a diverged
//!   tail can overlap a stale slice, and a conflict copy can repeat either; each is one row.
//! - `reason`: [`STALE_EPOCH`], [`MALFORMED`] (folder records, from chain resolution),
//!   [`DIVERGED_TAIL`] (this server's unexported records that lost), [`SUPERSEDED`] (applied
//!   records whose line lost to a newer epoch and were rewound, `super::rewind`, with the
//!   rows of the descendant branches that cascaded with them, `super::cascade`),
//!   [`CONFLICT_COPY`] (records of a foreign file that the DB does not hold),
//!   [`ID_COLLISION`] (the records of a branch whose `branch` record lost its id to another
//!   server's, `super::branches` and `super::aside`; kept as reason once set, and current
//!   while the folder's verdicts set the record aside, like `SUPERSEDED`: the record that
//!   holds the id can itself be set aside later, and the loser's branch comes back; an
//!   unexported row of such a branch, which exists nowhere else, is a `DIVERGED_TAIL`),
//!   [`IMPORTED`] (origin `foreign`: records of segment files moved out of the folder and read
//!   back by `fin project recover --import`, `crate::recover`; they exist nowhere else, so
//!   they stay current like `DIVERGED_TAIL`, and keep their reason). Rebuild
//!   adds `SALVAGED` (origin `salvage`): the unexported records of the database it
//!   replaced, which exist nowhere else; the old database's own quarantine rows come along
//!   with their reason, origin `salvage`. `DIVERGED_TAIL` and `SUPERSEDED` are kept as reason, origin and
//!   detail once set: they say what this database did with the record, which a later verdict
//!   on the same bytes (`STALE_EPOCH` for the losing line in the folder) does not change.
//! - `origin`: `folder`, `local` or `foreign`; `detail`: the segment hash (and why it is
//!   malformed), the tail's commit epoch and seq, or the foreign file's path.
//! - `current`: the row is set aside *now*. Folder and foreign verdicts are per poll, so each
//!   pass recomputes it from that pass's verdicts ([`flag_current`]); a row whose record is
//!   no longer reported (the file was removed, or a later poll applied it) stays, with its
//!   bytes, but is not counted. A diverged tail exists nowhere else, so it stays current until
//!   its record is in `txs` again. A `SUPERSEDED` row is current while the folder's verdicts
//!   still set its record aside (as for every other follower, which never applied it).
//!   Rows are never deleted by import (nor by anything else but a rebuild, which starts a
//!   new table), so `id` (`INTEGER PRIMARY KEY`, frozen in migration 0001 without
//!   `AUTOINCREMENT`) only grows: `crate::counters::Counters::raise_to_quarantine` relies on
//!   it as its watermark.
//! - A record the DB holds in `txs` is never quarantined (a harmless duplicate).

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use rusqlite::{Connection, params};

use super::ForeignFile;
use super::ctx::Ctx;
use crate::StoreError;
use crate::conflict_names::Provider;
use crate::db::hash_bytes;
use crate::segment::Segment;

pub(super) const STALE_EPOCH: &str = "STALE_EPOCH";
pub(super) const MALFORMED: &str = "MALFORMED";
pub(super) const DIVERGED_TAIL: &str = "DIVERGED_TAIL";
pub(super) const SUPERSEDED: &str = "SUPERSEDED";
pub(super) const CONFLICT_COPY: &str = "CONFLICT_COPY";
/// The records of a branch whose `branch` record lost its id to another server's (plan 4B
/// Decision 33). Like `SUPERSEDED`: kept once set, current while this pass's verdicts set
/// the record aside.
pub(super) const ID_COLLISION: &str = "ID_COLLISION";
/// Records of segment files moved out of the folder and read back (`fin project recover
/// --import`, plan 4B Decision 29): kept once set and always current, like `DIVERGED_TAIL`.
pub(crate) const IMPORTED: &str = "IMPORTED";

pub(super) struct Quarantined<'a> {
    pub(super) tx_hash: &'a str,
    pub(super) branch: &'a str,
    pub(super) origin: &'a str,
    pub(super) reason: &'a str,
    pub(super) detail: &'a str,
    pub(super) bytes: &'a [u8],
    pub(super) now: i64,
}

/// Inserts `q`, or marks its row current again. A `DIVERGED_TAIL`, `SUPERSEDED`, `SALVAGED`,
/// `ID_COLLISION` or `IMPORTED` row keeps its reason, origin and detail (module docs); otherwise the latest reason wins. Writes nothing when the row
/// is already current with the same reason and origin.
pub(super) fn upsert(conn: &Connection, q: &Quarantined) -> Result<(), StoreError> {
    conn.execute(
        "INSERT INTO quarantine (tx_hash, branch, origin, reason, detail, record_bytes,
                                 added_at, seen_at, current)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7, 1)
         ON CONFLICT(tx_hash) DO UPDATE SET
             current = 1,
             seen_at = excluded.seen_at,
             origin = CASE WHEN quarantine.reason IN ('DIVERGED_TAIL', 'SUPERSEDED', 'SALVAGED', 'ID_COLLISION', 'IMPORTED')
                           THEN quarantine.origin ELSE excluded.origin END,
             detail = CASE WHEN quarantine.reason IN ('DIVERGED_TAIL', 'SUPERSEDED', 'SALVAGED', 'ID_COLLISION', 'IMPORTED')
                           THEN quarantine.detail ELSE excluded.detail END,
             reason = CASE WHEN quarantine.reason IN ('DIVERGED_TAIL', 'SUPERSEDED', 'SALVAGED', 'ID_COLLISION', 'IMPORTED')
                           THEN quarantine.reason ELSE excluded.reason END
         WHERE quarantine.current = 0
            OR (quarantine.reason NOT IN ('DIVERGED_TAIL', 'SUPERSEDED', 'SALVAGED', 'ID_COLLISION', 'IMPORTED')
                AND (quarantine.reason != excluded.reason OR quarantine.origin != excluded.origin))",
        params![q.tx_hash, q.branch, q.origin, q.reason, q.detail, q.bytes, q.now],
    )?;
    Ok(())
}

/// Sets `current` from this pass's verdicts: the rows in `current` (folder and foreign
/// records reported now) and every diverged tail, salvaged and imported record, minus records
/// `txs` holds.
pub(super) fn flag_current(
    conn: &Connection,
    current: &BTreeSet<String>,
) -> Result<(), StoreError> {
    conn.execute(
        "UPDATE quarantine SET current = (reason IN ('DIVERGED_TAIL', 'SALVAGED', 'IMPORTED'))
         WHERE current != (reason IN ('DIVERGED_TAIL', 'SALVAGED', 'IMPORTED'))",
        [],
    )?;
    let mut stmt =
        conn.prepare("UPDATE quarantine SET current = 1 WHERE tx_hash = ?1 AND current = 0")?;
    for h in current {
        stmt.execute(params![h])?;
    }
    conn.execute(
        "UPDATE quarantine SET current = 0
         WHERE current = 1 AND tx_hash IN (SELECT tx_hash FROM txs)",
        [],
    )?;
    Ok(())
}

/// Records set aside now.
pub(super) fn count(conn: &Connection) -> Result<u32, StoreError> {
    let n: i64 = conn.query_row(
        "SELECT COUNT(*) FROM quarantine WHERE current = 1",
        [],
        |r| r.get(0),
    )?;
    Ok(u32::try_from(n).unwrap_or(u32::MAX))
}

pub(super) struct Foreign {
    pub(super) files: Vec<ForeignFile>,
    /// `txHash`es set aside from foreign files this pass.
    pub(super) hashes: BTreeSet<String>,
}

/// Lists the foreign files and imports the records of those that decode as segments:
/// records the DB holds are harmless duplicates, the others go to quarantine
/// ([`CONFLICT_COPY`]).
pub(super) fn foreign(
    ctx: &mut Ctx,
    project_dir: &Path,
    found: Vec<(PathBuf, Option<Provider>, Option<Arc<Segment>>)>,
) -> Result<Foreign, StoreError> {
    let mut out = Foreign {
        files: vec![],
        hashes: BTreeSet::new(),
    };
    for (path, conflict_copy, seg) in found {
        let mut file = ForeignFile {
            path,
            conflict_copy,
            known: 0,
            quarantined: 0,
        };
        if let Some(seg) = seg {
            let detail = file
                .path
                .strip_prefix(project_dir)
                .unwrap_or(&file.path)
                .to_string_lossy()
                .into_owned();
            for bytes in &seg.records {
                let tx_hash = hash_bytes(bytes);
                if ctx.has_hash(&tx_hash)? {
                    file.known += 1;
                    continue;
                }
                upsert(
                    ctx.txn,
                    &Quarantined {
                        tx_hash: &tx_hash,
                        branch: &seg.header.branch,
                        origin: "foreign",
                        reason: CONFLICT_COPY,
                        detail: &detail,
                        bytes,
                        now: ctx.now,
                    },
                )?;
                file.quarantined += 1;
                out.hashes.insert(tx_hash);
            }
        }
        out.files.push(file);
    }
    Ok(out)
}
