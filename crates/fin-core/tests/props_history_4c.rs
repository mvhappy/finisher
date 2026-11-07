//! Undo/redo property over the 4C edits (spec §4.6 prop 4; plan 4C Task 26 property 4): the
//! 4B property with cross-track and same-track ripple moves and splits. The model, its two
//! oracles, its exclusions and the driver are shared with `props_history` (`undo_model`,
//! whose docs describe them).

mod common;
#[path = "undo_model/mod.rs"]
mod undo_model;

use common::memlog::MemLog;
use common::moves::{move_to, spans_on};
use common::*;
use fin_core::{Edge, InsertMode, ItemSplit};
use proptest::prelude::*;
use undo_model::driver::{act_4c, run_script, script};
use undo_model::{frames, genesis};

proptest! {
    #![proptest_config(prop_config(64))]
    #[test]
    fn undo_redo_matches_reference_model_4c(script in script(act_4c)) {
        run_script(&genesis(), &script)?;
    }
}

/// Suspected engine defect (the model transcribes it; module docs, *Exclusions*): an undo of a
/// move restores the item as it was before the move, so a later edit of it by another author
/// is dropped without a conflict. An undo of a trim after another author's trim of the same end
/// is `SAME_EDGE` (spec §4.3); the restore puts the end back too, so this should be one.
#[test]
#[ignore = "suspected engine defect: an undo of a move drops a later trim of the item"]
fn undo_of_move_keeps_a_later_trim() {
    let mut log = MemLog::new(with_clips(&[2, 2, 2]));
    let to_t2 = move_to("c1", "t2", secs(0), InsertMode::Ripple);
    log.commit("main", "m1", "alice", vec![to_t2]);
    log.commit("main", "t1", "bob", vec![trim("c1", Edge::End, frames(-9))]);
    // Today: Ok, and c1 is back on t1 at 2 s: bob's trim is gone.
    let Err(e) = log.undo("main", "alice", "u1") else {
        panic!("no conflict: t1 {:?}", spans_on(&log.state("main"), 0));
    };
    assert_eq!(e.details["reason"], "SAME_EDGE", "{e:?}");
}

/// The same defect after a split of the moved item: the undo restores the left part at its
/// length before the split while the right part stays, so the right part's media plays twice.
#[test]
#[ignore = "suspected engine defect: an undo of a move after a split duplicates media"]
fn undo_of_move_after_a_split_keeps_the_split() {
    let mut log = MemLog::new(with_clips(&[2, 2, 2]));
    let to_t2 = move_to("c1", "t2", secs(0), InsertMode::Ripple);
    log.commit("main", "m1", "alice", vec![to_t2]);
    let split = ItemSplit {
        item: "c1".into(),
        offset: secs(1),
        id: "$x".into(),
    };
    log.commit("main", "s1", "bob", vec![split.into()]);
    // Today: Ok, with c1 (2 s, on t1) and the right part c4 (1 s, on t2) both showing its
    // second half.
    let Err(e) = log.undo("main", "alice", "u1") else {
        let p = log.state("main");
        panic!(
            "no conflict: t1 {:?}, t2 {:?}",
            spans_on(&p, 0),
            spans_on(&p, 1)
        );
    };
    assert_eq!(e.details["reason"], "SAME_EDGE", "{e:?}");
}
