//! Which source records a merge copies (spec §4.4; plan 4B Decisions 19 and 34).
//!
//! A merge is **transitive** (Decision 34): the candidates are the source's records that carry
//! edits (`edit`, `undo`, `redo`, `cherryPick`, `revert`, and `merge` copies brought in from
//! other branches) after the common point of the two lineages, in order. "Already there" is
//! judged by origin **root** ([`Roots`]), so it holds however a record travelled. Left out
//! silently: a record the target already holds ([`Roots::present`]: its root is on the
//! target's lineage, or a target record copies the same root). Such a record a merge skipped
//! earlier is offered again, since nothing copied it.
//!
//! Left out and reported as `DROPPED`: a record whose origin root is dropped for the target
//! ([`super::dropped_for`], sub-project 5 spec §8.3), whatever its kind. Not a conflict: what
//! depends on it is `DEPENDS_ON_SKIPPED` ([`super::plan_merge`]).
//!
//! Left out and reported as `UNDOES_TARGET_RECORDS`: a source `undo`, `redo` or `revert` (or a
//! copy of one) whose units hold a record that originates on the target
//! ([`Roots::undoes_target`]), since re-running it would undo the target's own edits.

use std::collections::BTreeSet;

use super::LogRec;
use super::origin::Roots;
use crate::rebase::reasons;
use crate::record::RecordKind;

/// The candidates of a merge: indexes into the source path, in order, and the excluded ones
/// with their reason ([`reasons::DROPPED`], [`reasons::UNDOES_TARGET_RECORDS`]).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Candidates {
    pub picks: Vec<usize>,
    pub excluded: Vec<(usize, &'static str)>,
}

/// The candidates among `source_path` (the source lineage's records after the common point,
/// oldest first), with `roots` ([`Roots::new`] over both paths) and `dropped`, the origin roots
/// dropped for the target ([`super::dropped_for`]). Module docs.
pub fn candidates(source_path: &[LogRec], roots: &Roots, dropped: &BTreeSet<String>) -> Candidates {
    let mut out = Candidates::default();
    for (i, rec) in source_path.iter().enumerate() {
        let kind = rec.record.kind;
        if !(kind.is_content() || kind == RecordKind::Merge) || roots.present[i] {
            continue;
        }
        if dropped.contains(&roots.source[i]) {
            out.excluded.push((i, reasons::DROPPED));
        } else if roots.undoes_target[i] {
            out.excluded.push((i, reasons::UNDOES_TARGET_RECORDS));
        } else {
            out.picks.push(i);
        }
    }
    out
}
