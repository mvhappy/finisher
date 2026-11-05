//! Which source records a merge copies (spec §4.4; plan 4B Decision 19).
//!
//! The candidates are the source's **content** records (`edit`, `undo`, `redo`, `cherryPick`,
//! `revert`) after the common point of the two lineages, in order. Left out silently:
//! - a record whose txId is already the origin of a `merge` or `cherryPick` record on the
//!   target's lineage after the common point (merged or picked before: a merge skipped earlier
//!   offers its skipped records again);
//! - a `cherryPick` whose origin is on the target's lineage (the target holds the original);
//! - `merge` records (a merge group is never re-merged; its records are not content).
//!
//! Left out and reported as `UNDOES_TARGET_RECORDS`: a source `undo`, `redo` or `revert` whose
//! unit is tied to the target, since re-running it would undo the target's own edits. A unit
//! is tied when it is a merge group with a record that originates on the target, a
//! `cherryPick` of a target record, or a record excluded this way (an undo of such a redo,
//! say).

use std::collections::HashSet;

use super::LogRec;
use crate::rebase::reasons;
use crate::record::RecordKind;

/// The candidates of a merge: indexes into the source path, in order, and the excluded ones
/// with their reason ([`reasons::UNDOES_TARGET_RECORDS`]).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Candidates {
    pub picks: Vec<usize>,
    pub excluded: Vec<(usize, &'static str)>,
}

/// The candidates among `source_path` (the source lineage's records after the common point,
/// oldest first) against `target_path` (the target's, likewise). Module docs.
pub fn candidates(source_path: &[LogRec], target_path: &[LogRec]) -> Candidates {
    let on_target: HashSet<&str> = target_path
        .iter()
        .map(|r| r.record.tx_id.as_str())
        .collect();
    let copied: HashSet<&str> = target_path
        .iter()
        .filter(|r| matches!(r.record.kind, RecordKind::Merge | RecordKind::CherryPick))
        .filter_map(|r| r.record.origin.as_deref())
        .collect();
    let from_target = |r: &LogRec| {
        r.record
            .origin
            .as_deref()
            .is_some_and(|o| on_target.contains(o))
    };
    // Unit ids (txIds, mergeIds) whose undo would undo target records.
    let mut tied: HashSet<&str> = HashSet::new();
    let mut out = Candidates::default();
    for (i, rec) in source_path.iter().enumerate() {
        let r = &rec.record;
        match r.kind {
            RecordKind::Merge => {
                if from_target(rec)
                    && let Some(m) = r.merge_id.as_deref()
                {
                    tied.insert(m);
                }
                continue;
            }
            RecordKind::CherryPick if from_target(rec) => {
                tied.insert(&r.tx_id);
                continue;
            }
            k if !k.is_content() => continue,
            _ => {}
        }
        if copied.contains(r.tx_id.as_str()) {
            continue;
        }
        let ties = match r.kind {
            RecordKind::Undo => r.undoes.iter().flatten().any(|u| tied.contains(u.as_str())),
            RecordKind::Redo => r.redoes.as_deref().is_some_and(|u| tied.contains(u)),
            RecordKind::Revert => r.reverts.as_deref().is_some_and(|u| tied.contains(u)),
            _ => false,
        };
        if ties {
            tied.insert(&r.tx_id);
            out.excluded.push((i, reasons::UNDOES_TARGET_RECORDS));
        } else {
            out.picks.push(i);
        }
    }
    out
}
