//! Migration 0001: the sub-project 2 layout → hashed commit records (sub-project 4 spec §3.1).
//!
//! - Each old tx becomes the [`fin_core::Record`] a 4A commit of the same tx writes
//!   ([`crate::db::new_record`]): `seq` = its `rev`, `intent` = its stored JSON, `resolved` from
//!   its bindings, and its stored `inverse`, `bindings` and `committed_at`. The record is
//!   encoded once; those bytes are stored and hashed, chained per branch in rev order
//!   (`parent_hash` null for the first), at epoch 0 and not exported.
//! - `txs` is rebuilt (the old one is renamed aside, the v1 table created, filled, the old one
//!   dropped), so the result has exactly the v1 columns.
//! - The tables 4A adds (`segments`, `lease_state`, `quarantine`) are created, the header gets
//!   `application_id`, and `meta` gets `formatVersion` and `minReaderVersion`.
//! - Snapshots are kept as they are; they load through `fin_core::upcast_snapshot`.
//!
//! The SQL here is frozen at schema version 1: later versions change the schema in later steps.

use std::collections::{BTreeMap, HashMap};

use fin_core::{Op, Tx, encode_record};
use rusqlite::{Transaction, params};

use crate::StoreError;
use crate::db::{hash_bytes, new_record, set_meta, to_u64};
use crate::migrations::APPLICATION_ID;

const V1_TABLES: &str = "
ALTER TABLE txs RENAME TO txs_sp2;
CREATE TABLE txs (
    seq INTEGER NOT NULL,
    branch TEXT NOT NULL,
    tx_id TEXT NOT NULL UNIQUE,
    kind TEXT NOT NULL,
    record_bytes BLOB NOT NULL,
    tx_hash TEXT NOT NULL,
    parent_hash TEXT,
    epoch INTEGER NOT NULL,
    exported INTEGER NOT NULL,
    diff TEXT NOT NULL,
    schema_version INTEGER NOT NULL,
    author TEXT NOT NULL,
    committed_at INTEGER NOT NULL,
    PRIMARY KEY (branch, seq)
);
CREATE TABLE segments (
    branch TEXT NOT NULL,
    hash TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    epoch INTEGER NOT NULL,
    first_seq INTEGER NOT NULL,
    last_seq INTEGER NOT NULL,
    pack INTEGER NOT NULL,
    replaces TEXT NOT NULL,
    state TEXT NOT NULL
);
CREATE TABLE lease_state (key TEXT PRIMARY KEY, value TEXT NOT NULL);
CREATE TABLE quarantine (
    id INTEGER PRIMARY KEY,
    origin TEXT NOT NULL,
    reason TEXT NOT NULL,
    record_bytes BLOB NOT NULL,
    added_at INTEGER NOT NULL
);
";

type OldRow = (i64, String, String, String, String, String, i64);

pub(super) fn run(txn: &Transaction<'_>) -> Result<(), StoreError> {
    txn.execute_batch(V1_TABLES)?;
    let mut select = txn.prepare(
        "SELECT rev, branch, json, diff, inverse, bindings, committed_at
         FROM txs_sp2 ORDER BY branch, rev",
    )?;
    let rows = select.query_map([], |r| {
        Ok((
            r.get(0)?,
            r.get(1)?,
            r.get(2)?,
            r.get(3)?,
            r.get(4)?,
            r.get(5)?,
            r.get(6)?,
        ))
    })?;
    let mut insert = txn.prepare(
        "INSERT INTO txs (seq, branch, tx_id, kind, record_bytes, tx_hash, parent_hash, epoch,
                          exported, diff, schema_version, author, committed_at)
         VALUES (?1, ?2, ?3, 'edit', ?4, ?5, ?6, 0, 0, ?7, ?8, ?9, ?10)",
    )?;
    let mut last_hash: HashMap<String, String> = HashMap::new();
    for row in rows {
        let (rev, branch, json, diff, inverse, bindings, committed_at): OldRow = row?;
        let intent: Tx = serde_json::from_str(&json)?;
        let inverse: Vec<Op> = serde_json::from_str(&inverse)?;
        let bindings: BTreeMap<String, String> = serde_json::from_str(&bindings)?;
        let parent = last_hash.get(&branch).cloned();
        let record = new_record(
            to_u64(rev)?,
            intent,
            inverse,
            bindings,
            parent,
            committed_at,
        )?;
        let bytes = encode_record(&record);
        let hash = hash_bytes(&bytes);
        insert.execute(params![
            rev,
            branch,
            record.tx_id,
            bytes,
            hash,
            record.parent_hash,
            diff,
            record.schema_version,
            record.author,
            record.committed_at,
        ])?;
        last_hash.insert(branch, hash);
    }
    drop(insert);
    drop(select);
    txn.execute_batch("DROP TABLE txs_sp2;")?;
    txn.pragma_update(None, "application_id", APPLICATION_ID)?;
    set_meta(txn, "formatVersion", "1")?;
    set_meta(txn, "minReaderVersion", "1")?;
    Ok(())
}
