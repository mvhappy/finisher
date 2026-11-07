//! Differential stale-vs-fresh property for move, slide, link and transition edits (4C Tasks
//! 9–10 review; extends 4B's `rebase_props` intent model): our intents realised at the base
//! and rebased over committed ones (some undone) agree with the same intents realised at the
//! head, or conflict as the spec allows; committed records replay exactly and undo exactly; a
//! successful stale rebase replays and undoes exactly and keeps transitions at cuts. The model
//! is [`keyed4c`]; `props_rebase_4c` runs it over every 4C edit.

mod common;
#[path = "keyed4c/mod.rs"]
mod keyed4c;

use common::prop_config;
use keyed4c::{base, check, intent, lens};
use proptest::prelude::*;

proptest! {
    #![proptest_config(prop_config(256))]
    #[test]
    fn stale_and_fresh_agree_with_moves_links_transitions(
        lens in lens(),
        linked in any::<bool>(),
        trs in any::<bool>(),
        committed in prop::collection::vec(
            (prop::collection::vec(intent(), 1..3), any::<bool>()),
            0..5,
        ),
        mine in prop::collection::vec(intent(), 1..4),
    ) {
        check(&base(&lens, linked, trs), &committed, &mine)?;
    }
}
