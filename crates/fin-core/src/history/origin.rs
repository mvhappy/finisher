//! Origin chains (plan 4B Decisions 18, 33): a `merge` or `cherryPick` record names the record
//! it copies in `origin`; following `origin` while the log holds the record gives the chain's
//! **root**, the record first written. Copies of one root are the same edit, wherever they
//! went, so "already present" is judged by root.

use super::{LogRec, LogView};
use crate::error::CoreError;

/// How deep an origin chain is followed (a loop in stored links ends there).
const MAX_CHAIN: usize = 1_000;

/// The root of `rec`'s origin chain: its txId, and the root record when the log holds it.
/// The txId is the last `origin` named, also when the log does not hold that record.
pub fn root(log: &dyn LogView, rec: &LogRec) -> Result<(String, Option<LogRec>), CoreError> {
    let mut id = rec.record.tx_id.clone();
    let mut held = Some(rec.clone());
    for _ in 0..MAX_CHAIN {
        let Some(o) = held.as_ref().and_then(|r| r.record.origin.clone()) else {
            break;
        };
        held = log.find(&o)?;
        id = o;
    }
    Ok((id, held))
}

/// The txId of `rec`'s origin root ([`root`]).
pub fn root_of(log: &dyn LogView, rec: &LogRec) -> Result<String, CoreError> {
    Ok(root(log, rec)?.0)
}

/// Each record of `target` that binds ids: its origin root and the ids it bound (the ids a
/// re-run of the same root may meet again: `ALREADY_PRESENT`, Decision 18).
pub fn target_roots(
    log: &dyn LogView,
    target: &[LogRec],
) -> Result<Vec<(String, Vec<String>)>, CoreError> {
    target
        .iter()
        .filter(|t| !t.record.bindings.is_empty())
        .map(|t| {
            Ok((
                root_of(log, t)?,
                t.record.bindings.values().cloned().collect(),
            ))
        })
        .collect()
}
