//! Dismissed groups (review of Task 16): rows a person has looked at and given up on (a group
//! that can never be recovered, `problems` non-empty) stop being listed and counted on this
//! server. Their bytes stay in quarantine and `tx.get` still answers for them. The set is
//! local (`meta` `recoverDismissed`, a JSON list of `txHash`es): it is not exported, so other
//! servers keep their own view, and a rebuild starts without it.

use std::collections::HashSet;

use rusqlite::Connection;

use crate::StoreError;
use crate::db::{meta, set_meta};

const KEY: &str = "recoverDismissed";

/// The dismissed `txHash`es.
pub(crate) fn load(conn: &Connection) -> Result<HashSet<String>, StoreError> {
    Ok(match meta(conn, KEY)? {
        Some(v) => serde_json::from_str(&v)?,
        None => HashSet::new(),
    })
}

/// Adds `hashes` to the dismissed set; returns how many were not in it.
pub(crate) fn add(conn: &Connection, hashes: &[String]) -> Result<usize, StoreError> {
    let mut set = load(conn)?;
    let n = hashes.iter().filter(|h| set.insert((*h).clone())).count();
    if n > 0 {
        let mut all: Vec<&String> = set.iter().collect();
        all.sort();
        set_meta(conn, KEY, &serde_json::to_string(&all)?)?;
    }
    Ok(n)
}
