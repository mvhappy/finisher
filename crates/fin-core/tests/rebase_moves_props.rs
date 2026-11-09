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

/// The shrunk case of a workspace run (spec §4.3 *self relocation*): t1 holds c1, c2, c3 and
/// the head ripples c2 to t1's end, so c3 now starts where c2 did (frame 10). Ours slides c3
/// back a frame, then ripple-moves c4 before c2, written at frame 10. That position maps to 10
/// (c2's range collapses to its start), but at the head our rebased slide put c3's start at 9
/// without a shift, so 10 is inside c3 and the re-run refuses `INSERT_INSIDE_CLIP`, unblamed:
/// the head only shifted c3 (`keyed4c::self_drift`).
#[test]
fn slide_then_move_at_moved_anchor_refuses_unblamed() {
    use keyed4c::Intent::{Move, Slide};
    let lens = [vec![10, 10, 10], vec![], vec![], vec![10]];
    let committed = [(
        vec![Move {
            track: 0,
            clip: 4,
            to: 0,
            before: None,
            overwrite: false,
        }],
        false,
    )];
    let mine = [
        Slide {
            track: 0,
            clip: 2,
            frames: -1,
        },
        Move {
            track: 3,
            clip: 0,
            to: 0,
            before: Some(4),
            overwrite: false,
        },
    ];
    check(&base(&lens, false, false), &committed, &mine).unwrap();
}

/// The shrunk case of a 30k-case run (`keyed4c::anchor_drift`): `c1` on t1 is linked with `c3`
/// on t2, and the head ripple-moves `c4` to its own end on t4, which records a removal of
/// `[0, 11)` and an insert at 0. Ours ripple-moves `c3` before `c4`, written at 0; that maps past
/// the re-inserted `c4` to frame 11, and the rebased move carries `c1` by the same delta into
/// `c2` on t1, so the re-run refuses `INSERT_INSIDE_CLIP`, unblamed.
#[test]
fn partner_past_relocated_anchor_refuses_unblamed() {
    use keyed4c::Intent::Move;
    let lens = [vec![10, 12], vec![10], vec![], vec![11]];
    let committed = [(
        vec![Move {
            track: 3,
            clip: 0,
            to: 3,
            before: None,
            overwrite: false,
        }],
        false,
    )];
    let mine = [Move {
        track: 1,
        clip: 0,
        to: 3,
        before: Some(0),
        overwrite: false,
    }];
    check(&base(&lens, true, false), &committed, &mine).unwrap();
}

/// The shrunk case of a 100k-case run (`keyed4c::anchor_drift`): t2 holds `c1` and `c2`, and
/// the head ripple-moves `c2` to its own end, recording a removal of `[10, 20)` and an insert
/// at 10. Ours ripple-moves `c3` from t3 before `c2`, written at frame 10, then sets a
/// transition between `c1` and `c3`. The move maps past the re-inserted `c2` (frame 20), so the
/// transition's cut is gone and the re-run refuses `NOT_ADJACENT`, unblamed.
#[test]
fn transition_past_relocated_anchor_refuses_unblamed() {
    use keyed4c::Intent::{Move, TrSet};
    let lens = [vec![], vec![10, 10], vec![10], vec![]];
    let committed = [(
        vec![Move {
            track: 1,
            clip: 5,
            to: 1,
            before: None,
            overwrite: false,
        }],
        false,
    )];
    let mine = [
        Move {
            track: 2,
            clip: 0,
            to: 1,
            before: Some(1),
            overwrite: false,
        },
        TrSet { track: 1, clip: 0 },
    ];
    check(&base(&lens, false, false), &committed, &mine).unwrap();
}

/// The shrunk case of a 100k-case run (spec §4.3 *lifted*, `keyed4c::lifted_gap`): t4 holds
/// `c1`, `c2` and `c3`; the head sets a transition between `c1` and `c2`, then slides `c3` back
/// a frame, so t4 ends at frame 29 and no shift is recorded. Ours ripple-moves `c1` to t4's end,
/// written at frame 30, then slides it back two frames. The move keeps 30 and leaves a frame's
/// gap after `c3`, so the slide has no touching neighbour and is refused `SLIDE_BLOCKED`, with
/// nothing blamed: both committed records touched what ours names.
#[test]
fn slide_after_lifted_end_refuses_unblamed() {
    use keyed4c::Intent::{Move, Slide, TrSet};
    let lens = [vec![], vec![], vec![], vec![10, 10, 10]];
    let committed = [
        (vec![TrSet { track: 3, clip: 2 }], false),
        (
            vec![Slide {
                track: 3,
                clip: 2,
                frames: -1,
            }],
            false,
        ),
    ];
    let mine = [
        Move {
            track: 3,
            clip: 3,
            to: 3,
            before: None,
            overwrite: false,
        },
        Slide {
            track: 3,
            clip: 2,
            frames: -2,
        },
    ];
    check(&base(&lens, false, false), &committed, &mine).unwrap();
}

/// Open seed `cc 5cee31d6`: the head inserts a clip after `c1` on t1, then overwrite-moves it
/// to t2. Ours inserts after `c1`, then sets a transition between `c1` and our clip.
#[test]
fn seed_5cee31d6_insert_after_lifted_concurrent_then_transition() {
    use keyed4c::Intent::{Insert, Move, TrSet};
    let lens = [vec![10], vec![], vec![], vec![]];
    let committed = [
        (
            vec![Insert {
                track: 0,
                before: None,
                len: 1,
            }],
            false,
        ),
        (
            vec![Move {
                track: 0,
                clip: 5,
                to: 1,
                before: None,
                overwrite: true,
            }],
            false,
        ),
    ];
    let mine = [
        Insert {
            track: 0,
            before: None,
            len: 3,
        },
        TrSet { track: 0, clip: 0 },
    ];
    check(&base(&lens, false, false), &committed, &mine).unwrap();
}

/// Open seed `cc 1c600fdf`: `c1` (t1) is linked with `c4` (t2). The head ripple-moves `c2` to
/// t4, then overwrite-moves it back to t1 at zero. Ours ripple-moves `c4` to t4's end.
#[test]
fn seed_1c600fdf_move_to_end_of_track_emptied_by_overwrite() {
    use keyed4c::Intent::Move;
    let lens = [vec![10, 11, 10], vec![10], vec![], vec![]];
    let committed = [
        (
            vec![Move {
                track: 0,
                clip: 1,
                to: 3,
                before: None,
                overwrite: false,
            }],
            false,
        ),
        (
            vec![Move {
                track: 3,
                clip: 0,
                to: 0,
                before: None,
                overwrite: true,
            }],
            false,
        ),
    ];
    let mine = [Move {
        track: 1,
        clip: 0,
        to: 3,
        before: None,
        overwrite: false,
    }];
    check(&base(&lens, true, false), &committed, &mine).unwrap();
}

/// Open seed `cc 3130272a`: `c1` (t1) is linked with `c2` (t2). The head inserts a clip on the
/// empty t3, then overwrite-moves it to t1. Ours ripple-moves `c1` to t3's end.
#[test]
fn seed_3130272a_move_to_track_emptied_by_overwrite_move() {
    use keyed4c::Intent::{Insert, Move};
    let lens = [vec![10], vec![10], vec![], vec![]];
    let committed = [
        (
            vec![Insert {
                track: 2,
                before: None,
                len: 1,
            }],
            false,
        ),
        (
            vec![Move {
                track: 2,
                clip: 0,
                to: 0,
                before: None,
                overwrite: true,
            }],
            false,
        ),
    ];
    let mine = [Move {
        track: 0,
        clip: 0,
        to: 2,
        before: None,
        overwrite: false,
    }];
    check(&base(&lens, true, false), &committed, &mine).unwrap();
}

/// Open seed `cc aa390d30`: the head trims `c1`'s end back a frame, then ripple-moves t4's
/// first clip to its own end. Ours inserts on t4 before its second clip, then overwrite-moves
/// `c3` (t2) before t4's first clip.
#[test]
fn seed_aa390d30_overwrite_move_before_relocated_first_clip() {
    use keyed4c::Intent::{Insert, Move, Trim};
    let lens = [vec![10, 12], vec![10], vec![], vec![11, 10, 10]];
    let committed = [
        (
            vec![Trim {
                track: 0,
                clip: 0,
                start: false,
                frames: -1,
                mode: 0,
            }],
            false,
        ),
        (
            vec![Move {
                track: 3,
                clip: 3,
                to: 3,
                before: None,
                overwrite: false,
            }],
            false,
        ),
    ];
    let mine = [
        Insert {
            track: 3,
            before: Some(1),
            len: 10,
        },
        Move {
            track: 1,
            clip: 0,
            to: 3,
            before: Some(0),
            overwrite: true,
        },
    ];
    check(&base(&lens, true, false), &committed, &mine).unwrap();
}

/// Open seed `cc 1477599b`: t1 holds `c1`, `c2` (`c1` linked with t2's `c3`). The head
/// ripple-moves `c1` to its own end. Ours overwrite-moves `c2` before t2's first clip, then
/// deletes t2's second clip.
#[test]
fn seed_1477599b_overwrite_move_before_partner_then_delete() {
    use keyed4c::Intent::{Delete, Move};
    let lens = [vec![15, 38], vec![10, 29], vec![], vec![]];
    let committed = [(
        vec![Move {
            track: 0,
            clip: 0,
            to: 0,
            before: None,
            overwrite: false,
        }],
        false,
    )];
    let mine = [
        Move {
            track: 0,
            clip: 5,
            to: 1,
            before: Some(0),
            overwrite: true,
        },
        Delete { track: 1, clip: 1 },
    ];
    check(&base(&lens, true, false), &committed, &mine).unwrap();
}

/// Open seed `cc c65a9221`: the head slides t2's second clip, inserts before `c1` on t1, then
/// slides t2's second clip back. Ours ripple-moves t2's first clip before t3's first, then
/// overwrite-moves `c1` to t2's end.
#[test]
fn seed_c65a9221_slides_then_moves_across_partners() {
    use keyed4c::Intent::{Insert, Move, Slide};
    let lens = [vec![24], vec![11, 34], vec![25, 12], vec![]];
    let committed = [
        (
            vec![
                Slide {
                    track: 1,
                    clip: 5,
                    frames: -1,
                },
                Insert {
                    track: 0,
                    before: Some(0),
                    len: 29,
                },
            ],
            false,
        ),
        (
            vec![Slide {
                track: 1,
                clip: 1,
                frames: -3,
            }],
            false,
        ),
    ];
    let mine = [
        Move {
            track: 1,
            clip: 2,
            to: 2,
            before: Some(0),
            overwrite: false,
        },
        Move {
            track: 0,
            clip: 0,
            to: 1,
            before: None,
            overwrite: true,
        },
    ];
    check(&base(&lens, true, false), &committed, &mine).unwrap();
}
