//! What two servers must agree on once both have followed the folder: every branch's head
//! (seq and txHash), a hash of every branch's state, and a hash of the derived branch index
//! (the `branches` and `record_links` tables, plan 4B Decisions 5–6).

use std::collections::BTreeMap;

use fin_store::Store;
use serde_json::{Value, json};

use crate::db::Db;

fn hash(b: &[u8]) -> String {
    format!("blake3:{}", blake3::hash(b).to_hex())
}

/// `{heads: {branch: [seq, txHash]}, stateHash, indexHash, index}` (or `{error}`).
pub fn of(store: &Store, db: &Db) -> Value {
    match try_of(store, db) {
        Ok(v) => v,
        Err(e) => json!({ "error": e }),
    }
}

fn try_of(store: &Store, db: &Db) -> Result<Value, String> {
    let mut heads = BTreeMap::new();
    let mut states = BTreeMap::new();
    for b in store.branches(true).map_err(|e| e.to_string())? {
        let (seq, p) = store.snapshot(&b.id).map_err(|e| e.to_string())?;
        heads.insert(b.id.clone(), json!([seq, db.hash_at(&b.id, seq)]));
        states.insert(b.id, p);
    }
    let states = serde_json::to_vec(&states).map_err(|e| e.to_string())?;
    let index = db.index_rows()?;
    let ib = serde_json::to_vec(&index).map_err(|e| e.to_string())?;
    Ok(json!({
        "heads": heads, "stateHash": hash(&states), "indexHash": hash(&ib), "index": index,
    }))
}
