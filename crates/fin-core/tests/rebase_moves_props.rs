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

/// The shrunk case of a workspace run: the head ripples `c4` in after `c3`, at our
/// overwrite move's anchor on t2, so the rebased move takes `c2` (and its partner `c1` on t1)
/// ten frames later than where it was written. Our later ripple move of `c1` to its
/// own end maps through no shift (an overwrite move records none) and lands inside `c1`, and
/// the re-run refuses it, unblamed (`keyed4c::partner_drift`).
#[test]
fn partner_of_overwrite_move_refuses_unblamed() {
    use keyed4c::Intent::Move;
    let lens = [vec![31], vec![10, 10], vec![], vec![10]];
    let committed = [(
        vec![Move {
            track: 3,
            clip: 0,
            to: 1,
            before: None,
            overwrite: false,
        }],
        false,
    )];
    let mine = [
        Move {
            track: 1,
            clip: 0,
            to: 1,
            before: None,
            overwrite: true,
        },
        Move {
            track: 0,
            clip: 0,
            to: 0,
            before: None,
            overwrite: false,
        },
    ];
    check(&base(&lens, true, false), &committed, &mine).unwrap();
}
