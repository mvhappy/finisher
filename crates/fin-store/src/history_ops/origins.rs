//! What a history operation's records stand for (sub-project 7A Decision 27): the txIds a
//! merge or cherry-pick copied, an undo undid or a revert took back, each with the records its
//! `origin` chain leads to (a copy of a copy, back to the record first written), so a reader
//! such as the agent outcome tracker joins a result to the transactions it acted on without
//! re-deriving the store's links.

use std::collections::HashSet;

use fin_core::Link;
use rusqlite::{Connection, params};

use crate::{StoreError, db};

/// How deep an origin chain is followed (a loop in stored links ends there).
const MAX_CHAIN: usize = 1_000;

/// `ids` and every record their `origin` chains lead to, first seen first, each once. A txId
/// the database does not hold is kept, and its chain ends there.
pub(crate) fn chains(
    conn: &Connection,
    ids: impl IntoIterator<Item = String>,
) -> Result<Vec<String>, StoreError> {
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    for id in ids {
        let mut next = Some(id);
        for _ in 0..MAX_CHAIN {
            let Some(id) = next.take() else { break };
            if !seen.insert(id.clone()) {
                break;
            }
            next = db::find_tx(conn, &id)?.and_then(|c| match c.link.as_deref() {
                Some(Link::Merge { origin, .. } | Link::CherryPick { origin, .. }) => {
                    Some(origin.clone())
                }
                _ => None,
            });
            out.push(id);
        }
    }
    Ok(out)
}

/// The records of merge group `merge_id`, in branch and seq order.
pub(crate) fn merge_group(conn: &Connection, merge_id: &str) -> Result<Vec<String>, StoreError> {
    let mut stmt = conn.prepare_cached(
        "SELECT tx_id FROM record_links WHERE kind = 'merge' AND merge_id = ?1 ORDER BY branch, seq",
    )?;
    let rows = stmt.query_map(params![merge_id], |row| row.get(0))?;
    Ok(rows.collect::<Result<_, _>>()?)
}

/// Undo units as records: a unit that is a merge group (named by its mergeId, which is never a
/// record's txId) becomes the group's records; any other is a record already.
pub(crate) fn units(conn: &Connection, ids: &[String]) -> Result<Vec<String>, StoreError> {
    let mut out = Vec::with_capacity(ids.len());
    for id in ids {
        let group = merge_group(conn, id)?;
        if group.is_empty() {
            out.push(id.clone());
        } else {
            out.extend(group);
        }
    }
    Ok(out)
}
