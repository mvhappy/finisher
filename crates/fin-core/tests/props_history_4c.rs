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
use fin_core::{Edge, InsertMode, ItemSplit, Op, ParamSet, ParamValue};
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

/// An undo of a move is `item.remove`, then `item.restore` of the item as it was. The restore
/// puts the whole item back, its end edge included, so after another author's trim of that end
/// it is `SAME_EDGE` (spec §4.3, *as built*), not a silent loss of the trim. Found by this
/// property (it used to pass, with c1 back on t1 untrimmed: bob's trim was gone).
#[test]
fn undo_of_move_keeps_a_later_trim() {
    let mut log = MemLog::new(with_clips(&[2, 2, 2]));
    let to_t2 = move_to("c1", "t2", secs(0), InsertMode::Ripple);
    log.commit("main", "m1", "alice", vec![to_t2]);
    log.commit("main", "t1", "bob", vec![trim("c1", Edge::End, frames(-9))]);
    let Err(e) = log.undo("main", "alice", "u1") else {
        panic!("no conflict: t1 {:?}", spans_on(&log.state("main"), 0));
    };
    assert_eq!(e.details["reason"], "SAME_EDGE", "{e:?}");
}

/// The same after a split of the moved item: the split moved its end edge, so the restore is
/// `SAME_EDGE`. It used to pass, restoring the left part at its length before the split while
/// the right part stayed, so the right part's media played twice.
#[test]
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

/// The restore meets a later change of the item's `start` or of a parameter the same way:
/// `SAME_PARAM`, and nothing is written.
#[test]
fn undo_of_move_after_a_later_move_or_param_is_same_param() {
    let opacity: Op = ParamSet {
        item: "c1".into(),
        path: "opacity".into(),
        value: Some(ParamValue::Number(0.5)),
    }
    .into();
    for later in [move_to("c1", "t1", secs(0), InsertMode::Ripple), opacity] {
        let mut log = MemLog::new(with_clips(&[2, 2, 2]));
        let to_t2 = move_to("c1", "t2", secs(0), InsertMode::Ripple);
        log.commit("main", "m1", "alice", vec![to_t2]);
        log.commit("main", "b1", "bob", vec![later.clone()]);
        let before = log.state("main");
        let e = log.undo("main", "alice", "u1").expect_err("a conflict");
        assert_eq!(e.details["reason"], "SAME_PARAM", "{later:?}: {e:?}");
        assert_eq!(e.details["againstTx"], "b1", "{later:?}: {e:?}");
        assert_eq!(log.state("main"), before, "{later:?}");
    }
}

/// A later edit that was undone again (a mirrored pair) is not in the way: the undo of the
/// move applies and puts c1 back first on t1, at its length.
#[test]
fn undo_of_move_after_an_undone_trim_applies() {
    let mut log = MemLog::new(with_clips(&[2, 2, 2]));
    let to_t2 = move_to("c1", "t2", secs(0), InsertMode::Ripple);
    log.commit("main", "m1", "alice", vec![to_t2]);
    log.commit("main", "t1", "bob", vec![trim("c1", Edge::End, frames(-9))]);
    log.undo("main", "bob", "u0").unwrap();
    log.undo("main", "alice", "u1").unwrap();
    let p = log.state("main");
    assert_eq!(
        spans_on(&p, 0),
        [
            ("c1".to_string(), secs(0), secs(2)),
            ("c2".to_string(), secs(2), secs(2)),
            ("c3".to_string(), secs(4), secs(2)),
        ]
    );
    assert!(spans_on(&p, 1).is_empty());
}

/// The merge case of the same rule (spec §4.3, *as built*): bob deletes c1 on br1 and undoes
/// that, while alice trims c1 on main. The copy of bob's undo would restore c1 as bob deleted
/// it, without alice's trim; it is `SAME_EDGE`, blamed on her trim, and the merge writes
/// nothing. It used to merge, dropping the trim.
#[test]
fn merged_delete_and_restore_keeps_a_target_trim() {
    let mut log = MemLog::new(with_clips(&[2, 2, 2]));
    log.fork("br1", "main", 0);
    log.commit(
        "main",
        "t1",
        "alice",
        vec![trim("c1", Edge::End, frames(-9))],
    );
    log.commit("br1", "d1", "bob", vec![delete("c1")]);
    log.undo("br1", "bob", "u1").unwrap();
    let before = log.state("main");
    let e = log
        .merge("br1", "main", "carol", "m1", false)
        .expect_err("a conflict");
    assert_eq!(e.details["reason"], "SAME_EDGE", "{e:?}");
    assert_eq!(e.details["againstTx"], "t1", "{e:?}");
    assert_eq!(log.state("main"), before);
}
