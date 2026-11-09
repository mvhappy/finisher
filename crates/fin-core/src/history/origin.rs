//! Origin chains (plan 4B Decisions 18, 33): a `merge` or `cherryPick` record names the record
//! it copies in `origin`; following `origin` while the log holds the record gives the chain's
//! **root**, the record first written. Copies of one root are the same edit, wherever they
//! went, so "already present" is judged by root.

use std::collections::{HashMap, HashSet};

use super::{Lineage, LogRec, LogView, StackRec};
use crate::error::CoreError;
use crate::model::Project;
use crate::record::RecordKind;
use serde_json::json;

/// How many `origin` links a chain may have. Copies of copies are rare (a merge of a merge of a
/// branch …); the bound only stops a damaged log from costing unbounded reads.
pub const MAX_CHAIN: usize = 100_000;

/// `DB_ERROR` for an origin chain that loops back to a record it named (`originCycle`) or is
/// longer than [`MAX_CHAIN`] (`originChainTooLong`): the log holds links no write makes.
pub(super) fn chain_error(reason: &str, tx_id: &str, at: &str) -> CoreError {
    CoreError::new(
        "DB_ERROR",
        format!("the origin chain of record {tx_id} cannot be followed ({reason} at {at})"),
    )
    .with_details(json!({ "reason": reason, "txId": tx_id, "at": at, "max": MAX_CHAIN }))
    .with_fix("run `fin project verify`; the log's stored links are damaged")
}

/// The root of `rec`'s origin chain: its txId, and the root record when the log holds it.
/// The txId is the last `origin` named, also when the log does not hold that record. A chain
/// that names a record twice, or has more than [`MAX_CHAIN`] links, is `DB_ERROR`
/// ([`chain_error`]).
pub fn root(log: &dyn LogView, rec: &LogRec) -> Result<(String, Option<LogRec>), CoreError> {
    let start = rec.record.tx_id.as_str();
    let mut id = rec.record.tx_id.clone();
    let mut held = Some(rec.clone());
    let mut seen = HashSet::from([id.clone()]);
    while let Some(o) = held.as_ref().and_then(|r| r.record.origin.clone()) {
        if !seen.insert(o.clone()) {
            return Err(chain_error("originCycle", start, &o));
        }
        if seen.len() > MAX_CHAIN + 1 {
            return Err(chain_error("originChainTooLong", start, &o));
        }
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

/// Whether lineage `l` holds the row `seq` of `branch`.
pub(super) fn holds(l: &Lineage, branch: &str, seq: u64) -> bool {
    l.segs.iter().any(|(b, after, through)| {
        b == branch && seq > *after && seq <= through.unwrap_or(super::merge::TO_HEAD)
    })
}

/// The origin roots of a merge's two paths (records after the common point, oldest first),
/// and what they say about each source record:
///
/// - `on_target[j]`: the target's lineage holds source record `j`'s root (it originates on
///   the target);
/// - `present[j]`: the target already holds `j`'s edit: its root is on the target's lineage,
///   or a target-path record has the same root (a merge or pick of the same root there);
/// - `undoes_target[j]`: `j` (or the record it copies) is an `undo`, `redo` or `revert`
///   whose units include a record that originates on the target, directly or through
///   another such record (an undo of a redo of an undo of a merge from the target):
///   re-running it would undo the target's own edits (`UNDOES_TARGET_RECORDS`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Roots {
    pub source: Vec<String>,
    pub target: Vec<String>,
    pub on_target: Vec<bool>,
    pub present: Vec<bool>,
    pub undoes_target: Vec<bool>,
}

impl Roots {
    /// The roots of `source` and `target`, `target_l` being the target's lineage.
    pub fn new(
        log: &dyn LogView,
        target_l: &Lineage,
        source: &[LogRec],
        target: &[LogRec],
    ) -> Result<Roots, CoreError> {
        let known = Known::new(log, [source, target]);
        let log: &dyn LogView = &known;
        let target_roots: Vec<String> = target
            .iter()
            .map(|t| root_of(log, t))
            .collect::<Result<_, _>>()?;
        let copied: HashSet<&str> = target_roots.iter().map(String::as_str).collect();
        let mut ties = Ties {
            log,
            target_l,
            clear: HashSet::new(),
        };
        let mut out = Roots {
            target: target_roots.clone(),
            ..Roots::default()
        };
        for rec in source {
            let (id, held) = root(log, rec)?;
            let on = held
                .as_ref()
                .is_some_and(|r| holds(target_l, &r.branch, r.seq));
            out.present.push(on || copied.contains(id.as_str()));
            out.on_target.push(on);
            out.undoes_target
                .push(ties.inverts_target(held.as_ref().unwrap_or(rec))?);
            out.source.push(id);
        }
        Ok(out)
    }
}

/// Which records undo records that originate on a target ([`Roots::undoes_target`]).
struct Ties<'a> {
    log: &'a dyn LogView,
    target_l: &'a Lineage,
    /// Unit ids known to hold no such record (searched to the end).
    clear: HashSet<String>,
}

impl Ties<'_> {
    /// Whether `rec` (a root record) inverts a unit with a record from the target, directly
    /// or through units that invert such units (an undo of a redo of an undo …). A search
    /// over the units, with a work list rather than recursion: a chain of thousands of
    /// undo/redo cycles is that deep.
    fn inverts_target(&mut self, rec: &LogRec) -> Result<bool, CoreError> {
        let mut todo: Vec<String> = units(rec);
        let mut seen: HashSet<String> = HashSet::new();
        while let Some(id) = todo.pop() {
            if self.clear.contains(&id) || !seen.insert(id.clone()) {
                continue;
            }
            let mut recs = self.log.merge_group(&id)?;
            if recs.is_empty() {
                recs.extend(self.log.find(&id)?);
            }
            for rec in &recs {
                let (_, held) = root(self.log, rec)?;
                let base = held.as_ref().unwrap_or(rec);
                if holds(self.target_l, &base.branch, base.seq) {
                    return Ok(true);
                }
                todo.extend(units(base));
            }
        }
        // Every unit reached was searched to the end.
        self.clear.extend(seen);
        Ok(false)
    }
}

/// The units `rec` inverts: an undo's, a redo's undo, a revert's group.
fn units(rec: &LogRec) -> Vec<String> {
    let r = &rec.record;
    match r.kind {
        RecordKind::Undo => r.undoes.clone().unwrap_or_default(),
        RecordKind::Redo => r.redoes.iter().cloned().collect(),
        RecordKind::Revert => r.reverts.iter().cloned().collect(),
        _ => Vec::new(),
    }
}

/// `log`, answering [`LogView::find`] from records already read (a merge's two paths) before
/// asking it: the origins of most copies on one path are records on the other (plan 4B
/// Task 20). The paths are `log`'s own records, so the answers are the same.
struct Known<'a> {
    log: &'a dyn LogView,
    by_tx: HashMap<&'a str, &'a LogRec>,
}

impl<'a> Known<'a> {
    fn new<const N: usize>(log: &'a dyn LogView, paths: [&'a [LogRec]; N]) -> Known<'a> {
        let by_tx = paths
            .into_iter()
            .flatten()
            .map(|r| (r.record.tx_id.as_str(), r))
            .collect();
        Known { log, by_tx }
    }
}

impl LogView for Known<'_> {
    fn lineage(&self, branch: &str) -> Result<Lineage, CoreError> {
        self.log.lineage(branch)
    }

    fn records(&self, branch: &str, after: u64, through: u64) -> Result<Vec<LogRec>, CoreError> {
        self.log.records(branch, after, through)
    }

    fn stack_records(
        &self,
        branch: &str,
        after: u64,
        through: u64,
    ) -> Result<Vec<StackRec>, CoreError> {
        self.log.stack_records(branch, after, through)
    }

    fn head(&self, branch: &str) -> Result<(u64, Project), CoreError> {
        self.log.head(branch)
    }

    fn state_at(&self, branch: &str, seq: u64) -> Result<Project, CoreError> {
        self.log.state_at(branch, seq)
    }

    fn find(&self, tx_id: &str) -> Result<Option<LogRec>, CoreError> {
        match self.by_tx.get(tx_id) {
            Some(r) => Ok(Some((*r).clone())),
            None => self.log.find(tx_id),
        }
    }

    fn merge_group(&self, merge_id: &str) -> Result<Vec<LogRec>, CoreError> {
        self.log.merge_group(merge_id)
    }

    fn is_descendant(&self, a: &str, of: &str) -> Result<bool, CoreError> {
        self.log.is_descendant(a, of)
    }

    fn merged_away(&self, branch: &str) -> Result<HashSet<String>, CoreError> {
        self.log.merged_away(branch)
    }
}
