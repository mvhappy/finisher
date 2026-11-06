//! Read-only lookups in a store's database (a second connection; the store keeps the WAL).

use std::path::Path;
use std::time::Duration;

use rusqlite::{Connection, OptionalExtension};
use serde_json::{Value, json};

pub struct Db(Connection);

/// What a `txs` row says about one record.
pub struct Row {
    pub tx_hash: String,
    pub epoch: u64,
}

impl Db {
    pub fn open(path: &Path) -> Result<Db, String> {
        let c = Connection::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
        c.busy_timeout(Duration::from_secs(5))
            .map_err(|e| e.to_string())?;
        Ok(Db(c))
    }

    pub fn row(&self, tx_id: &str) -> Option<Row> {
        self.0
            .query_row(
                "SELECT tx_hash, epoch FROM txs WHERE tx_id = ?1",
                [tx_id],
                |r| {
                    Ok(Row {
                        tx_hash: r.get(0)?,
                        epoch: r.get(1)?,
                    })
                },
            )
            .optional()
            .ok()
            .flatten()
    }

    /// Every `main` row in seq order: `[seq, txId, txHash, epoch, exported, author]`.
    pub fn chain(&self) -> Result<Vec<Value>, String> {
        let mut st = self
            .0
            .prepare(
                "SELECT seq, tx_id, tx_hash, epoch, exported, author FROM txs \
                 WHERE branch = 'main' ORDER BY seq",
            )
            .map_err(|e| e.to_string())?;
        let rows = st
            .query_map([], |r| {
                Ok(json!([
                    r.get::<_, i64>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, i64>(3)?,
                    r.get::<_, i64>(4)? != 0,
                    r.get::<_, String>(5)?,
                ]))
            })
            .map_err(|e| e.to_string())?;
        rows.collect::<Result<_, _>>().map_err(|e| e.to_string())
    }

    /// Every row of every branch, by branch then seq: `[seq, txId, txHash, epoch, exported,
    /// author, branch, kind]`.
    pub fn all_rows(&self) -> Result<Vec<Value>, String> {
        let mut st = self
            .0
            .prepare(
                "SELECT seq, tx_id, tx_hash, epoch, exported, author, branch, kind FROM txs \
                 ORDER BY branch, seq",
            )
            .map_err(|e| e.to_string())?;
        let rows = st
            .query_map([], |r| {
                Ok(json!([
                    r.get::<_, i64>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, i64>(3)?,
                    r.get::<_, i64>(4)? != 0,
                    r.get::<_, String>(5)?,
                    r.get::<_, String>(6)?,
                    r.get::<_, String>(7)?,
                ]))
            })
            .map_err(|e| e.to_string())?;
        rows.collect::<Result<_, _>>().map_err(|e| e.to_string())
    }

    /// The txHash at `branch`'s `seq`, when a record is there.
    pub fn hash_at(&self, branch: &str, seq: u64) -> Option<String> {
        self.0
            .query_row(
                "SELECT tx_hash FROM txs WHERE branch = ?1 AND seq = ?2",
                rusqlite::params![branch, seq as i64],
                |r| r.get(0),
            )
            .optional()
            .ok()
            .flatten()
    }

    /// The derived branch index as rows: every `branches` row (by id) and every
    /// `record_links` row (by txId), each column as text.
    pub fn index_rows(&self) -> Result<Value, String> {
        let dump = |q: &str| -> Result<Vec<Value>, String> {
            let mut st = self.0.prepare(q).map_err(|e| e.to_string())?;
            let n = st.column_count();
            let rows = st
                .query_map([], |r| {
                    (0..n)
                        .map(|i| {
                            let v: rusqlite::types::Value = r.get(i)?;
                            Ok(match v {
                                rusqlite::types::Value::Null => Value::Null,
                                rusqlite::types::Value::Integer(x) => json!(x),
                                rusqlite::types::Value::Real(x) => json!(x),
                                rusqlite::types::Value::Text(x) => json!(x),
                                rusqlite::types::Value::Blob(x) => json!(x.len()),
                            })
                        })
                        .collect::<Result<Vec<_>, _>>()
                        .map(Value::Array)
                })
                .map_err(|e| e.to_string())?;
            rows.collect::<Result<_, _>>().map_err(|e| e.to_string())
        };
        Ok(json!({
            "branches": dump("SELECT * FROM branches ORDER BY id")?,
            "recordLinks": dump("SELECT * FROM record_links ORDER BY tx_id")?,
        }))
    }

    /// Every quarantine row: `{txHash, txId, seq, reason, origin, current}`.
    pub fn quarantine(&self) -> Result<Vec<Value>, String> {
        let mut st = self
            .0
            .prepare(
                "SELECT tx_hash, reason, origin, current, record_bytes FROM quarantine ORDER BY id",
            )
            .map_err(|e| e.to_string())?;
        let rows = st
            .query_map([], |r| {
                let bytes: Vec<u8> = r.get(4)?;
                let rec: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
                Ok(json!({
                    "txHash": r.get::<_, String>(0)?,
                    "txId": rec["txId"],
                    "seq": rec["seq"],
                    "author": rec["author"],
                    "reason": r.get::<_, String>(1)?,
                    "origin": r.get::<_, String>(2)?,
                    "current": r.get::<_, i64>(3)? != 0,
                }))
            })
            .map_err(|e| e.to_string())?;
        rows.collect::<Result<_, _>>().map_err(|e| e.to_string())
    }
}
