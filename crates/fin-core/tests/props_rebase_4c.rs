//! Prop 3 over every 4C edit (plan 4C Task 26): `stale_and_fresh_agree_4c`. Committed edits
//! and our stale edit come from inserts, deletes and trims in every mode (start trims
//! included), cross-track moves, slides, splits, slips, links, transitions, opacity sets,
//! keyframe set/remove, effect add/move, caption split/merge and font pins (the same pin on
//! both sides merges), on four media tracks and a caption track. When both the stale rebase and
//! the fresh realisation at the head succeed, the states are equal; a stale refusal where the
//! fresh edit applies is `SAME_EDGE`, `SAME_PARAM`, `REMOVED`, `FONT_PINNED` or a policy code a
//! committed record explains. Keyframe times and split offsets are content offsets (B3): the
//! fresh intent names the same content, moved by the head's start-edge changes. The model and
//! its known limitations are [`keyed4c`].

mod common;
#[path = "keyed4c/mod.rs"]
mod keyed4c;

use common::prop_config;
use keyed4c::{base_4c, check, intent_4c, intent_edges, intent_offsets, lens};
use proptest::prelude::*;

proptest! {
    #![proptest_config(prop_config(128))]
    #[test]
    fn stale_and_fresh_agree_4c(
        lens in lens(),
        linked in any::<bool>(),
        trs in any::<bool>(),
        extras in (0usize..4, any::<bool>(), any::<bool>()),
        committed in prop::collection::vec(
            (prop::collection::vec(intent_4c(), 1..3), any::<bool>()),
            0..5,
        ),
        mine in prop::collection::vec(intent_4c(), 1..4),
    ) {
        check(&base_4c(&lens, linked, trs, extras), &committed, &mine)?;
    }
}

proptest! {
    #![proptest_config(prop_config(64))]
    /// B3 in focus: committed edits move the start edges of the first clips of `t1` (some
    /// undone), and our keyframes and splits there name content offsets, which map with them.
    #[test]
    fn content_offsets_follow_start_edges(
        lens in prop::collection::vec(prop::collection::vec(10i64..40, 2..4), 4),
        extras in (Just(0usize), any::<bool>(), Just(false)),
        committed in prop::collection::vec(
            (prop::collection::vec(intent_edges(), 1..3), any::<bool>()),
            1..4,
        ),
        mine in prop::collection::vec(intent_offsets(), 1..3),
    ) {
        check(&base_4c(&lens, false, false, extras), &committed, &mine)?;
    }
}
