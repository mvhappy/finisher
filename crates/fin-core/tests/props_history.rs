//! Undo/redo property (spec §4.6 prop 4; plan 4B Task 20) over the 4B edits: gapless ripple
//! inserts at a clip boundary or the track end, deletes and end trims. The model, its two
//! oracles and the driver are shared with `props_history_4c` (`undo_model`, whose docs
//! describe them).

mod common;
#[path = "undo_model/mod.rs"]
mod undo_model;

use common::memlog::MemLog;
use common::*;
use proptest::prelude::*;
use undo_model::driver::{act_4b, run_script, script};
use undo_model::genesis;

proptest! {
    #![proptest_config(prop_config(64))]
    #[test]
    fn undo_redo_matches_reference_model(script in script(act_4b)) {
        let ran = run_script(&genesis(), &script)?;
        // 4B edits never leave a gap, so a run never stops early.
        prop_assert_eq!(ran, script.len());
    }
}

/// The tie-break the model allows (plan 4B Task 12 review): Bob deletes c3, Alice deletes c2,
/// Bob undoes, Alice undoes → `c1, c3, c2`. The recorded successors predict it.
#[test]
fn prosemirror_tie_break_is_the_models_order() {
    let mut log = MemLog::new(with_clips(&[10, 10, 10]));
    log.commit("main", "b1", "bob", vec![delete("c3")]);
    log.commit("main", "a1", "alice", vec![delete("c2")]);
    log.undo("main", "bob", "b2").unwrap();
    log.undo("main", "alice", "a2").unwrap();
    let order: Vec<String> = starts(&log.state("main"), 0)
        .into_iter()
        .map(|(id, _)| id)
        .collect();
    assert_eq!(order, ["c1", "c3", "c2"]);
}
