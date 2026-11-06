//! Origin chains (plan 4B Decisions 18, 33): a `merge` or `cherryPick` record names the record
//! it copies in `origin`; following `origin` while the log holds the record gives the chain's
//! **root**, the record first written. Copies of one root are the same edit, wherever they
//! went, so "already present" is judged by root.

use std::collections::{HashMap, HashSet};

use super::{Lineage, LogRec, LogView};
use crate::error::CoreError;
use crate::record::RecordKind;

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
        let target_roots: Vec<String> = target
            .iter()
            .map(|t| root_of(log, t))
            .collect::<Result<_, _>>()?;
        let copied: HashSet<&str> = target_roots.iter().map(String::as_str).collect();
        let mut ties = Ties {
            log,
            target_l,
            memo: HashMap::new(),
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
                .push(ties.inverts_target(held.as_ref().unwrap_or(rec), 0)?);
            out.source.push(id);
        }
        Ok(out)
    }
}

/// Which records undo records that originate on a target ([`Roots::undoes_target`]).
struct Ties<'a> {
    log: &'a dyn LogView,
    target_l: &'a Lineage,
    /// Unit id → whether it holds such a record.
    memo: HashMap<String, bool>,
}

impl Ties<'_> {
    /// Whether `rec` (a root record) inverts a unit with a record from the target.
    fn inverts_target(&mut self, rec: &LogRec, depth: usize) -> Result<bool, CoreError> {
        let r = &rec.record;
        let units: Vec<String> = match r.kind {
            RecordKind::Undo => r.undoes.clone().unwrap_or_default(),
            RecordKind::Redo => r.redoes.iter().cloned().collect(),
            RecordKind::Revert => r.reverts.iter().cloned().collect(),
            _ => return Ok(false),
        };
        for u in units {
            if self.unit(&u, depth + 1)? {
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// Whether unit `id` (a txId or a mergeId) holds a record from the target, or one that
    /// inverts such a unit.
    fn unit(&mut self, id: &str, depth: usize) -> Result<bool, CoreError> {
        if depth > MAX_CHAIN {
            return Ok(false);
        }
        if let Some(&v) = self.memo.get(id) {
            return Ok(v);
        }
        self.memo.insert(id.to_string(), false);
        let mut recs = self.log.merge_group(id)?;
        if recs.is_empty() {
            recs.extend(self.log.find(id)?);
        }
        let mut v = false;
        for rec in &recs {
            let (_, held) = root(self.log, rec)?;
            let base = held.as_ref().unwrap_or(rec);
            if holds(self.target_l, &base.branch, base.seq) || self.inverts_target(base, depth)? {
                v = true;
                break;
            }
        }
        self.memo.insert(id.to_string(), v);
        Ok(v)
    }
}
