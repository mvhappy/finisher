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
