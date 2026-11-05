//! The `branches` table: the branch index (plan 4B Decision 6). It is kept from `branch` and
//! `branchSet` records as they are appended ([`absorb`], from [`crate::write::WriteCtx::append`])
//! and imported (`crate::import::branches`), and [`crate::derived::recompute`] rebuilds it
//! from the log. A record that takes no effect (a `branch` record that lost an id collision,
//! one the index refuses) stays in the log as an inert row, marked in `record_links.ignored`. Branch references resolve
//! against it ([`resolve`]).

use fin_core::branch::{Absorbed, BranchIndex, BranchInfo};
use fin_core::{CoreError, Record, RecordKind};
use rusqlite::{Connection, OptionalExtension, Row, params};

use crate::db::{self, to_i64, to_u64};
use crate::{StoreError, branch_not_found};

const COLUMNS: &str = "id, name, parent, fork_seq, fork_hash, created_seq, created_by, \
                       created_at, record, epoch, archived, merged";

type RawRow = (BranchInfo, i64, Option<i64>, i64);

fn from_row(r: &Row<'_>) -> rusqlite::Result<RawRow> {
    let info = BranchInfo {
        id: r.get(0)?,
        name: r.get(1)?,
        parent: r.get(2)?,
        fork_seq: 0,
        fork_hash: r.get(4)?,
        created_seq: None,
        created_by: r.get(6)?,
        created_at: r.get(7)?,
        record: r.get(8)?,
        epoch: 0,
        archived: r.get::<_, i64>(10)? != 0,
        merged: r.get::<_, i64>(11)? != 0,
    };
    Ok((info, r.get(3)?, r.get(5)?, r.get(9)?))
}

fn finish((mut info, fork, created, epoch): RawRow) -> Result<BranchInfo, StoreError> {
    info.fork_seq = to_u64(fork)?;
    info.created_seq = created.map(to_u64).transpose()?;
    info.epoch = to_u64(epoch)?;
    Ok(info)
}

/// The whole index (plus `main` when its row is missing).
pub(crate) fn load(conn: &Connection) -> Result<BranchIndex, StoreError> {
    let mut stmt = conn.prepare_cached(&format!("SELECT {COLUMNS} FROM branches"))?;
    let rows = stmt.query_map([], from_row)?;
    let mut infos = vec![];
    for row in rows {
        infos.push(finish(row?)?);
    }
    Ok(BranchIndex::from_infos(infos))
}

/// The index row of `id`.
pub(crate) fn get(conn: &Connection, id: &str) -> Result<Option<BranchInfo>, StoreError> {
    let row = conn
        .prepare_cached(&format!("SELECT {COLUMNS} FROM branches WHERE id = ?1"))?
        .query_row(params![id], from_row)
        .optional()?;
    row.map(finish).transpose()
}

/// Writes `info` as its row (insert or replace).
pub(crate) fn save(conn: &Connection, info: &BranchInfo) -> Result<(), StoreError> {
    conn.prepare_cached(&format!(
        "INSERT OR REPLACE INTO branches ({COLUMNS})
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)"
    ))?
    .execute(params![
        info.id,
        info.name,
        info.parent,
        to_i64(info.fork_seq)?,
        info.fork_hash,
        info.created_seq.map(to_i64).transpose()?,
        info.created_by,
        info.created_at,
        info.record,
        to_i64(info.epoch)?,
        i64::from(info.archived),
        i64::from(info.merged),
    ])?;
    Ok(())
}

/// Replaces the whole table with `ix`.
pub(crate) fn replace_all(conn: &Connection, ix: &BranchIndex) -> Result<(), StoreError> {
    conn.execute("DELETE FROM branches", [])?;
    for (_, info) in ix.list() {
        save(conn, info)?;
    }
    Ok(())
}

/// Folds the record at `seq` of `branch` (its bytes hash to `tx_hash`) into the table as a
/// write appends it ([`BranchIndex::absorb`]). A write makes only new ids (the counters are
/// past every id the log and the quarantine hold), so an id collision here is damage.
pub(crate) fn absorb(
    conn: &Connection,
    branch: &str,
    seq: u64,
    rec: &Record,
    tx_hash: &str,
) -> Result<(), StoreError> {
    if !matches!(rec.kind, RecordKind::Branch | RecordKind::BranchSet) {
        return Ok(());
    }
    let mut ix = load(conn)?;
    match ix.absorb(branch, seq, rec, tx_hash)? {
        Absorbed::Nothing => Ok(()),
        Absorbed::Made(id) | Absorbed::Set(id) => {
            let info = ix.get(&id).ok_or_else(|| missing(&id))?;
            save(conn, info)
        }
        Absorbed::Replaced { id, .. } | Absorbed::Lost { id, .. } => Err(CoreError::new(
            "DB_ERROR",
            format!("record {} makes branch {id}, which exists", rec.tx_id),
        )
        .with_details(serde_json::json!({ "branch": id, "txId": rec.tx_id }))
        .into()),
    }
}

/// The `txHash` at `seq` on `branch`'s lineage as `ix` knows it (`None` at 0, or when that
/// row is missing): the branch's own row above its fork, an ancestor's below.
pub(crate) fn hash_on(
    conn: &Connection,
    ix: &BranchIndex,
    branch: &str,
    seq: u64,
) -> Result<Option<String>, StoreError> {
    let mut at = branch.to_string();
    for _ in 0..=ix.list().len() {
        if seq == 0 {
            return Ok(None);
        }
        match ix.get(&at) {
            Some(i) if i.parent.is_some() && seq <= i.fork_seq => {
                at = i.parent.clone().unwrap_or_default();
            }
            _ => break,
        }
    }
    Ok(conn
        .prepare_cached("SELECT tx_hash FROM txs WHERE branch = ?1 AND seq = ?2")?
        .query_row(params![at, to_i64(seq)?], |r| r.get(0))
        .optional()?)
}

/// Why the branch `info` (just folded from its `branch` record) cannot start where the
/// record says: its `forkHash` is not the parent's record at `forkSeq` (on the lineage of
/// `ix`, the index before the record). `None` when it can.
pub(crate) fn bad_fork(
    conn: &Connection,
    ix: &BranchIndex,
    info: &BranchInfo,
) -> Result<Option<String>, StoreError> {
    let parent = info.parent.as_deref().unwrap_or(fin_core::branch::MAIN);
    let at = hash_on(conn, ix, parent, info.fork_seq)?;
    Ok((at != info.fork_hash).then(|| {
        format!(
            "branch {} forks at seq {} of {parent} with forkHash {:?}, but that record is {at:?}",
            info.id, info.fork_seq, info.fork_hash
        )
    }))
}

/// Marks the `record_links` row of the record with `tx_hash` inert (`why`) or live (`None`):
/// a `branch` or `branchSet` record that took no effect on the index (status warns).
pub(crate) fn set_ignored(
    conn: &Connection,
    tx_hash: &str,
    why: Option<&str>,
) -> Result<(), StoreError> {
    conn.prepare_cached(
        "UPDATE record_links SET ignored = ?2
         WHERE tx_id IN (SELECT tx_id FROM txs WHERE tx_hash = ?1)",
    )?
    .execute(params![tx_hash, why])?;
    Ok(())
}

/// `ignored` for a `branch` record that lost id `id` to the record `winner`.
pub(crate) fn collision(id: &str, winner: &str) -> String {
    format!(
        "ID_COLLISION: branch {id} is held by branch record {}",
        crate::names::hash12(winner).unwrap_or(winner)
    )
}

/// `ignored` for a record the index refuses.
pub(crate) fn unfit(why: &str) -> String {
    format!("INVALID_FORMAT: {why}")
}

/// The branch reference `r` names (Decision 12): an id, an active display name, or
/// `<name>#<id>`. A branch with rows but no index row (made by hand in tests, or followed
/// before its branch record) is a root under its own id, as lineage reads take it.
/// `BRANCH_NOT_FOUND` otherwise.
pub(crate) fn resolve(conn: &Connection, r: &str) -> Result<BranchInfo, StoreError> {
    if let Some(info) = get(conn, r)? {
        return Ok(info);
    }
    if let Some(info) = load(conn)?.resolve(r) {
        return Ok(info.clone());
    }
    if db::latest_rev(conn, r)?.is_some() {
        return Ok(BranchInfo {
            id: r.into(),
            name: r.into(),
            ..BranchInfo::main()
        });
    }
    Err(branch_not_found(r).into())
}

/// A row the index should hold and does not: damage.
pub(crate) fn missing(id: &str) -> StoreError {
    CoreError::new(
        "DB_ERROR",
        format!("branch {id} has no row in the branch index"),
    )
    .with_details(serde_json::json!({ "branch": id }))
    .with_fix("rebuild the project database (fin project rebuild)")
    .into()
}
