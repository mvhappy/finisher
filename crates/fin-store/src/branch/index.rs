//! The `branches` table: the branch index (plan 4B Decision 6). It is kept from `branch` and
//! `branchSet` records as they are appended ([`absorb`], from [`crate::write::WriteCtx::append`]),
//! and [`crate::derived::recompute`] rebuilds it from the log. Branch references resolve
//! against it ([`resolve`]).

use fin_core::branch::{BranchIndex, BranchInfo};
use fin_core::{CoreError, Record, RecordKind};
use rusqlite::{Connection, OptionalExtension, Row, params};

use crate::db::{self, to_i64, to_u64};
use crate::{StoreError, branch_not_found};

const COLUMNS: &str = "id, name, parent, fork_seq, fork_hash, created_seq, created_by, \
                       created_at, archived, merged";

fn from_row(r: &Row<'_>) -> rusqlite::Result<(BranchInfo, i64, Option<i64>)> {
    let info = BranchInfo {
        id: r.get(0)?,
        name: r.get(1)?,
        parent: r.get(2)?,
        fork_seq: 0,
        fork_hash: r.get(4)?,
        created_seq: None,
        created_by: r.get(6)?,
        created_at: r.get(7)?,
        archived: r.get::<_, i64>(8)? != 0,
        merged: r.get::<_, i64>(9)? != 0,
    };
    Ok((info, r.get(3)?, r.get(5)?))
}

fn finish(
    (mut info, fork, created): (BranchInfo, i64, Option<i64>),
) -> Result<BranchInfo, StoreError> {
    info.fork_seq = to_u64(fork)?;
    info.created_seq = created.map(to_u64).transpose()?;
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
        "INSERT OR REPLACE INTO branches ({COLUMNS}) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)"
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

/// Folds the record at `seq` of `branch` into the table ([`BranchIndex::absorb`]); returns
/// the branch it changed. Other kinds than `branch` and `branchSet` read nothing.
pub(crate) fn absorb(
    conn: &Connection,
    branch: &str,
    seq: u64,
    rec: &Record,
) -> Result<Option<String>, StoreError> {
    if !matches!(rec.kind, RecordKind::Branch | RecordKind::BranchSet) {
        return Ok(None);
    }
    let mut ix = load(conn)?;
    let changed = ix.absorb(branch, seq, rec)?;
    if let Some(info) = changed.as_deref().and_then(|id| ix.get(id)) {
        save(conn, info)?;
    }
    Ok(changed)
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
