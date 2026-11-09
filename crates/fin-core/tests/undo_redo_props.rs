//! Determinism of undo and redo under the left-neighbour rule (owner decision 4 of
//! 2026-11-09; SP4 §4.3 *Placement of restored items*): alice makes ripple deletes and moves
//! while bob and carol ripple-move and delete the clips around them; then alice undoes as far
//! as she can (m undos) and redoes m times. Each redo gives back exactly the timeline from
//! before the undo it reverses, so the last gives the timeline from before the undos, also
//! when the neighbours were moved or removed (relocated restores included). Other authors act
//! only before the undos: a redo reads its anchors right after its undo, which the head equals
//! while nothing else came between.

mod common;

use common::memlog::MemLog;
use common::moves::move_to;
use common::*;
use fin_core::InsertMode;
use fin_core::rebase::placement::WhenLost;
use proptest::prelude::*;

#[derive(Debug, Clone)]
enum Act {
    /// A ripple delete of clip `k` (mod the clip count).
    Delete { who: usize, k: usize },
    /// A ripple move of clip `k` to before clip `at` (or the end).
    Move { who: usize, k: usize, at: usize },
}

fn act() -> impl Strategy<Value = Act> {
    prop_oneof![
        (0..3usize, 0..8usize).prop_map(|(who, k)| Act::Delete { who, k }),
        (0..3usize, 0..8usize, 0..9usize).prop_map(|(who, k, at)| Act::Move { who, k, at }),
    ]
}

const WHO: [&str; 3] = ["alice", "bob", "carol"];

proptest! {
    #![proptest_config(prop_config(256))]
    #[test]
    fn n_undos_then_n_redos_give_back_the_timeline(
        acts in prop::collection::vec(act(), 1..12),
        snap in any::<bool>(),
    ) {
        let mut log = MemLog::new(with_clips(&[10, 20, 10, 30, 10, 20, 10, 10]));
        log.when_lost = if snap { WhenLost::Snap } else { WhenLost::Refuse };
        for (i, a) in acts.iter().enumerate() {
            let p = log.state("main");
            let items = &p.timelines[0].tracks[0].items;
            if items.is_empty() {
                break;
            }
            let (who, op) = match *a {
                Act::Delete { who, k } => (who, delete(&items[k % items.len()].id)),
                Act::Move { who, k, at } => {
                    let c = &items[k % items.len()];
                    let pos = items.get(at).map_or_else(
                        || items.last().unwrap().end().unwrap(),
                        |n| n.start,
                    );
                    (who, move_to(&c.id, "t1", pos, InsertMode::Ripple))
                }
            };
            // A move into the item itself is refused; the script skips it.
            let _ = log.try_commit("main", &format!("e{i}"), WHO[who], vec![op]);
        }
        // m undos, as far as alice can go (a conflict or an empty stack stops them).
        let mut before = vec![log.state("main")];
        let mut m = 0;
        while log.undo("main", "alice", &format!("u{m}")).is_ok() {
            m += 1;
            before.push(log.state("main"));
            assert_well_formed(&log.state("main"));
        }
        // m redos: redo j reverses undo m - j, so it gives back the state before that undo.
        for j in 0..m {
            log.redo("main", "alice", &format!("r{j}"))
                .map_err(|e| TestCaseError::fail(format!("redo {j}: {e:?}")))?;
            let want = &before[m - 1 - j];
            prop_assert_eq!(&log.state("main").timelines, &want.timelines, "after redo {}", j);
        }
    }
}
