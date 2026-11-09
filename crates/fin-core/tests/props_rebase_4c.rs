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

/// The shrunk case of a random run (formerly *group changed*): `c1` on t1 and `c2` on t2 are
/// linked, and the head dissolves the group. Ours overwrite-trims `c2`'s end by a frame (where
/// written, that trims its partner `c1` too), then overwrite-moves `c1` to its own end, written at
/// frame 9. At the head the rebased trim leaves `c1` alone; the move is anchored at `c1`'s end, so
/// it goes to 10, as a fresh one does.
#[test]
fn unlink_then_trim_and_move_at_former_partner() {
    use keyed4c::Intent::{Move, Trim, Unlink};
    let lens = [vec![10], vec![10], vec![], vec![]];
    let committed = [(vec![Unlink { track: 0, clip: 0 }], false)];
    let mine = [
        Trim {
            track: 1,
            clip: 0,
            start: false,
            frames: -1,
            mode: 2,
        },
        Move {
            track: 0,
            clip: 0,
            to: 0,
            before: None,
            overwrite: true,
        },
    ];
    check(
        &base_4c(&lens, true, false, (0, false, false)),
        &committed,
        &mine,
    )
    .unwrap();
}

/// The shrunk case of a 30k-case run (formerly *lifted, for edges*): the head ripple-inserts `c2`
/// (four frames) after `c1` on t1, then overwrite-trims `c2`'s end by a frame (no shift). Ours
/// inserts after `c1`, written at 10. Through shifts it went past `c2`'s insert to 14, a frame past
/// `c2`'s new end. As an append (the end of t1's last item where written) it goes to t1's end
/// at the head: 13, `c2`'s end, as a fresh one.
#[test]
fn trim_of_concurrent_insert_leaves_gap() {
    use keyed4c::Intent::{Insert, Trim};
    let lens = [vec![10], vec![], vec![], vec![]];
    let committed = [
        (
            vec![Insert {
                track: 0,
                before: None,
                len: 4,
            }],
            false,
        ),
        (
            vec![Trim {
                track: 0,
                clip: 5,
                start: false,
                frames: -1,
                mode: 2,
            }],
            false,
        ),
    ];
    let mine = [Insert {
        track: 0,
        before: None,
        len: 1,
    }];
    check(
        &base_4c(&lens, false, false, (0, false, false)),
        &committed,
        &mine,
    )
    .unwrap();
}

/// The shrunk case of a 30k-case run: the head ripple-moves `c2` from t2 to t1's end, after
/// `c1`. Ours splits `c2` a frame in, then ripple-moves `c1` to its own end, written at frame
/// 10: an append, so it goes to t1's end at the head, past both parts of `c2` (frame 20), as a
/// fresh move does.
#[test]
fn split_of_concurrent_item_is_passed_after_anchor() {
    use keyed4c::Intent::{Move, Split};
    let lens = [vec![10], vec![10], vec![], vec![]];
    let committed = [(
        vec![Move {
            track: 1,
            clip: 0,
            to: 0,
            before: None,
            overwrite: false,
        }],
        false,
    )];
    let mine = [
        Split {
            track: 1,
            clip: 0,
            eighths: 1,
        },
        Move {
            track: 0,
            clip: 0,
            to: 0,
            before: None,
            overwrite: false,
        },
    ];
    check(
        &base_4c(&lens, false, false, (0, false, false)),
        &committed,
        &mine,
    )
    .unwrap();
}

/// Open seed `cc 5b02bc6b` (2026-11-09): t3 holds `c2`, t4 holds `c3`; the head ripple-moves `c3`
/// to t3's end. Ours overwrite-trims `c2`'s end by a frame, then ripple-moves `c1` (t2) to t3's
/// end, written at 11 (`c2`'s end). Through shifts it went to 21, a frame past `c3`; anchored at
/// `c2`'s end, an append: it goes to t3's end at the head, past the ripple-moved `c3`: 20, as a
/// fresh one.
#[test]
fn seed_5b02bc6b_trim_then_move_to_track_filled_since() {
    use keyed4c::Intent::{Move, Trim};
    let lens = [vec![], vec![10], vec![10], vec![10]];
    let committed = [(
        vec![Move {
            track: 3,
            clip: 0,
            to: 2,
            before: None,
            overwrite: false,
        }],
        false,
    )];
    let mine = [
        Trim {
            track: 2,
            clip: 0,
            start: false,
            frames: 1,
            mode: 2,
        },
        Move {
            track: 1,
            clip: 0,
            to: 2,
            before: None,
            overwrite: false,
        },
    ];
    check(
        &base_4c(&lens, false, false, (0, false, false)),
        &committed,
        &mine,
    )
    .unwrap();
}

/// Plan review finding 1: the head inserts `c2` after `c1`, then overwrite-trims it and undoes
/// the trim. Ours appends after `c1`: both sides go to t1's end, after `c2`.
#[test]
fn undone_trim_of_concurrent_insert_says_nothing() {
    use keyed4c::Intent::{Insert, Trim};
    let lens = [vec![10], vec![], vec![], vec![]];
    let committed = [
        (
            vec![Insert {
                track: 0,
                before: None,
                len: 4,
            }],
            false,
        ),
        (
            vec![Trim {
                track: 0,
                clip: 1,
                start: false,
                frames: -1,
                mode: 2,
            }],
            true,
        ),
    ];
    let mine = [Insert {
        track: 0,
        before: None,
        len: 1,
    }];
    check(
        &base_4c(&lens, false, false, (0, false, false)),
        &committed,
        &mine,
    )
    .unwrap();
}

/// Plan review finding 1: the head ripple-moves `c1` to the end of t1 and undoes it, then
/// ripple-moves `c2` before `c1`. Ours appends after `c2` (t1's last item where written): both
/// sides go to t1's end at the head, after `c1`.
#[test]
fn undone_ripple_move_does_not_make_an_item_concurrent() {
    use keyed4c::Intent::{Insert, Move};
    let lens = [vec![10, 10], vec![], vec![], vec![]];
    let committed = [
        (
            vec![Move {
                track: 0,
                clip: 0,
                to: 0,
                before: None,
                overwrite: false,
            }],
            true,
        ),
        (
            vec![Move {
                track: 0,
                clip: 1,
                to: 0,
                before: Some(0),
                overwrite: false,
            }],
            false,
        ),
    ];
    let mine = [Insert {
        track: 0,
        before: None,
        len: 3,
    }];
    check(
        &base_4c(&lens, false, false, (0, false, false)),
        &committed,
        &mine,
    )
    .unwrap();
}
