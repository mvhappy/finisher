//! Rebasing one stale transaction over one committed one (plan 4C Tasks 12–13).
#![allow(dead_code)]

use fin_core::rebase::Rebased;
use fin_core::{CommittedTx, CoreError, MemIds, Op, Project, rebase_tx};

use super::tx;

/// Commits `ops` on `p` as `tx_id` (rev 1).
pub fn committed(p: &Project, ops: Vec<Op>, tx_id: &str) -> (Project, CommittedTx) {
    let mut t = tx(ops);
    t.tx_id = tx_id.into();
    let t = fin_core::canonicalize(p, &t).unwrap();
    let a = fin_core::apply_tx(p, &t, &mut MemIds::after(p)).unwrap();
    let c = CommittedTx {
        rev: 1,
        tx: t,
        diff: a.diff,
        bindings: a.bindings,
        link: None,
    };
    (a.project, c)
}

/// `ours` (`tx-b`, made at `p`) rebased over `theirs` (`tx-a`, committed at `p`).
pub fn rebase_over(p: &Project, theirs: Vec<Op>, ours: Vec<Op>) -> Result<Rebased, CoreError> {
    let (head, c) = committed(p, theirs, "tx-a");
    let mut t = tx(ours);
    t.tx_id = "tx-b".into();
    rebase_tx(p, &head, &t, &[c], &mut MemIds::after(&head))
}

/// The reason of a `CONFLICT`.
pub fn conflict_reason(e: &CoreError) -> &str {
    assert_eq!(e.code, "CONFLICT", "{e:?}");
    e.details["reason"].as_str().unwrap()
}
