//! Exact per-track shift maps and mirrored mappings (`fin_core::rebase::map`, Decision 16).

use fin_core::rebase::{Assoc, Mapping, Recover, StepMap, Unmapped};
use fin_core::{Diff, Rational, Shift};
use proptest::prelude::*;

fn q(n: i64) -> Rational {
    Rational::from_int(n)
}

fn sh(track: &str, from: i64, by: i64) -> Shift {
    Shift {
        track: track.into(),
        from: q(from),
        by: q(by),
        op: 0,
    }
}

fn step(shifts: &[(&str, i64, i64)]) -> StepMap {
    StepMap::from_shifts(shifts.iter().map(|&(t, f, b)| sh(t, f, b)).collect())
}

fn mapping(maps: Vec<StepMap>, mirrors: &[(usize, usize)]) -> Mapping {
    let mut m = Mapping::new();
    for map in maps {
        m.push(map);
    }
    for &(a, b) in mirrors {
        m.set_mirror(a, b);
    }
    m
}

/// The 4A rule (`rebase::shift_position`): a position at or after a shift's `from` moves by `by`.
fn shift_position_4a(track: &str, pos: Rational, shifts: &[Shift]) -> Rational {
    let mut pos = pos;
    for s in shifts.iter().filter(|s| s.track == track) {
        if pos >= s.from {
            pos = pos.checked_add(s.by).unwrap();
        }
    }
    pos
}

/// `PROPTEST_CASES` overrides the per-block default.
fn cases(default: u32) -> u32 {
    std::env::var("PROPTEST_CASES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

fn config() -> ProptestConfig {
    ProptestConfig {
        cases: cases(256),
        ..ProptestConfig::default()
    }
}

fn rational(lo: i64, hi: i64) -> impl Strategy<Value = Rational> {
    (lo..hi, 1i64..4).prop_map(|(n, d)| Rational::new(n, d).unwrap())
}

fn assoc() -> impl Strategy<Value = Assoc> {
    prop_oneof![Just(Assoc::Before), Just(Assoc::After)]
}

fn shift_on(tracks: &'static [&'static str]) -> impl Strategy<Value = Shift> {
    (
        prop::sample::select(tracks),
        rational(0, 120),
        rational(-30, 30),
    )
        .prop_map(|(track, from, by)| Shift {
            track: track.to_string(),
            from,
            by,
            op: 0,
        })
}

fn step_map(tracks: &'static [&'static str]) -> impl Strategy<Value = StepMap> {
    prop::collection::vec(shift_on(tracks), 0..6).prop_map(StepMap::from_shifts)
}

// ---- StepMap basics ----

#[test]
fn insert_moves_positions_by_assoc() {
    let m = step(&[("t1", 10, 3)]);
    let at = |pos: i64, a: Assoc| m.map("t1", q(pos), a).unwrap();
    assert_eq!(at(9, Assoc::After).pos, q(9));
    assert_eq!(at(10, Assoc::After).pos, q(13));
    assert_eq!(at(10, Assoc::Before).pos, q(10));
    assert_eq!(at(11, Assoc::Before).pos, q(14));
    for a in [Assoc::Before, Assoc::After] {
        assert_eq!(at(10, a).deleted, None);
    }
}

#[test]
fn removal_collapses_inside_and_records_offsets() {
    // Removes [10, 13).
    let m = step(&[("t1", 13, -3)]);
    let at = |pos: i64, a: Assoc| m.map("t1", q(pos), a).unwrap();
    assert_eq!(at(9, Assoc::After).pos, q(9));
    assert_eq!(at(9, Assoc::After).deleted, None);
    assert_eq!(at(14, Assoc::Before).pos, q(11));
    assert_eq!(at(14, Assoc::Before).deleted, None);
    assert!(!at(10, Assoc::After).across && !at(13, Assoc::Before).across);
    let inside = at(12, Assoc::After);
    assert!(inside.across);
    assert_eq!(inside.pos, q(10));
    assert_eq!(
        inside.deleted,
        Some(Recover {
            shift: 0,
            offset: q(2)
        })
    );
    // Before: the start edge survives, the end edge is deleted (ProseMirror).
    assert_eq!(at(10, Assoc::Before).deleted, None);
    assert_eq!(at(10, Assoc::Before).pos, q(10));
    let end = at(13, Assoc::Before);
    assert_eq!(end.pos, q(10));
    assert_eq!(
        end.deleted,
        Some(Recover {
            shift: 0,
            offset: q(3)
        })
    );
}

#[test]
fn from_diff_uses_the_diff_shifts() {
    let diff = Diff {
        changes: vec![],
        shifts: vec![sh("t1", 4, 2), sh("t2", 0, -1)],
    };
    assert_eq!(StepMap::from_diff(&diff).shifts(), diff.shifts.as_slice());
}

#[test]
fn invert_reverses_and_negates() {
    let m = step(&[("t1", 10, 3), ("t2", 8, -2)]);
    assert_eq!(
        m.invert().unwrap().shifts(),
        &[sh("t2", 6, 2), sh("t1", 13, -3)]
    );
}

#[test]
fn other_tracks_are_untouched() {
    let m = step(&[("t1", 0, 5), ("t1", 20, -10), ("t1", 3, 0)]);
    for pos in [0, 3, 10, 15, 20, 40] {
        for a in [Assoc::Before, Assoc::After] {
            let r = m.map("t2", q(pos), a).unwrap();
            assert_eq!((r.pos, r.deleted), (q(pos), None));
        }
    }
}

#[test]
fn overflow_is_reported() {
    let m = step(&[("t1", 0, i64::MAX)]);
    let e = m.map("t1", q(i64::MAX), Assoc::After).unwrap_err();
    assert_eq!(e.code, "TIME_OVERFLOW");
    assert_eq!(
        mapping(vec![m], &[]).map("t1", q(i64::MAX), Assoc::After),
        Err(Unmapped::Overflow)
    );
    assert_eq!(
        step(&[("t1", i64::MAX, i64::MAX)])
            .invert()
            .unwrap_err()
            .code,
        "TIME_OVERFLOW"
    );
}

#[test]
fn start_edge_without_mirror_carries_on() {
    // c1 [0, 5), c2 [5, 10); a committed ripple delete of c2 removes [5, 10). A stale insert at
    // the end of c1 (= the start of c2) is a boundary, not a reference to c2: as in 4A, it maps
    // to 5, like one at 10. Only a position strictly inside c2 is lost.
    let m = mapping(vec![step(&[("t1", 10, -5)])], &[]);
    for a in [Assoc::Before, Assoc::After] {
        assert_eq!(m.map("t1", q(5), a), Ok(q(5)));
        assert_eq!(m.map("t1", q(10), a), Ok(q(5)));
        assert_eq!(m.map("t1", q(7), a), Err(Unmapped::Deleted { step: 0 }));
    }
    assert_eq!(m.map("t1", q(12), Assoc::After), Ok(q(7)));
}

#[test]
fn map_from_skips_earlier_maps() {
    let m = mapping(vec![step(&[("t1", 0, 5)]), step(&[("t1", 10, -3)])], &[]);
    assert_eq!(m.map("t1", q(12), Assoc::After), Ok(q(14)));
    assert_eq!(m.map_from(1, "t1", q(12), Assoc::After), Ok(q(9)));
    assert_eq!(m.map_from(2, "t1", q(12), Assoc::After), Ok(q(12)));
    // A mirror before `start` is never followed: 11 just moves with A.
    let m = mapping(
        vec![step(&[("t1", 13, -3)]), step(&[("t1", 10, 3)])],
        &[(0, 1)],
    );
    assert_eq!(m.map_from(1, "t1", q(11), Assoc::After), Ok(q(14)));
}

#[test]
#[should_panic(expected = "already mirrored")]
fn set_mirror_refuses_a_second_mirror() {
    let mut m = mapping(vec![StepMap::default(); 3], &[(0, 1)]);
    m.set_mirror(1, 2);
}

#[test]
fn recover_overflow_is_reported() {
    let rec = Recover {
        shift: 0,
        offset: q(1),
    };
    let ins = step(&[("t1", i64::MAX, 2)]);
    assert_eq!(ins.recover(rec, q(2)).unwrap_err().code, "TIME_OVERFLOW");
    // Removes [1, 3); 2 is recovered one into the mirror at i64::MAX.
    let m = mapping(vec![step(&[("t1", 3, -2)]), ins], &[(0, 1)]);
    assert_eq!(m.map("t1", q(2), Assoc::After), Err(Unmapped::Overflow));
}

// ---- Mirrors (Decision 16) ----

#[test]
fn insert_before_own_earlier_insert() {
    // Our tx inserts A (3 s) at 10, then B at 10, before A. A 2 s insert at 5 was committed.
    // B's position is mapped over [inverse(A), committed, A'] with A' mirroring inverse(A).
    let a = step(&[("t1", 10, 3)]);
    let committed = step(&[("t1", 5, 2)]);
    let a2 = step(&[("t1", 12, 3)]);
    let m = mapping(vec![a.invert().unwrap(), committed, a2], &[(0, 2)]);
    // B lands at A'.from, so it is still placed before A (which then ripples to 12 + |B|).
    assert_eq!(m.map("t1", q(10), Assoc::After), Ok(q(12)));
    // A position just past A keeps following A.
    assert_eq!(m.map("t1", q(13), Assoc::After), Ok(q(15)));
}

#[test]
fn start_edge_of_inverted_insert_recovers_offset_zero() {
    // inverse(A) removes [10, 14); with After the start edge is deleted with offset 0.
    let inv = step(&[("t1", 10, 4)]).invert().unwrap();
    let r = inv.map("t1", q(10), Assoc::After).unwrap();
    assert_eq!(r.pos, q(10));
    assert_eq!(
        r.deleted,
        Some(Recover {
            shift: 0,
            offset: q(0)
        })
    );
    // A committed removal of [2, 5) before it moves A' to 7: the edge resumes at 7 + 0.
    let maps = vec![inv.clone(), step(&[("t1", 5, -3)]), step(&[("t1", 7, 4)])];
    assert_eq!(
        mapping(maps.clone(), &[(0, 2)]).map("t1", q(10), Assoc::After),
        Ok(q(7))
    );
    // Without the mirror the edge is a boundary, not a reference: it carries on from 10, so
    // the (unmirrored) A' at 7 then pushes it past A' with After.
    assert_eq!(mapping(maps, &[]).map("t1", q(10), Assoc::After), Ok(q(11)));
}

#[test]
fn end_edge_with_after_is_not_deleted() {
    let inv = step(&[("t1", 10, 3)]).invert().unwrap();
    let r = inv.map("t1", q(13), Assoc::After).unwrap();
    assert_eq!((r.pos, r.deleted), (q(10), None));
    // It then follows the re-inserted A' with After: past its end.
    let m = mapping(vec![inv.clone(), step(&[("t1", 10, 3)])], &[(0, 1)]);
    assert_eq!(m.map("t1", q(13), Assoc::After), Ok(q(13)));
    // Before: the end edge is deleted with offset |by| and recovered at the end of A'.
    let m = mapping(
        vec![inv, step(&[("t1", 0, 1)]), step(&[("t1", 11, 3)])],
        &[(0, 2)],
    );
    assert_eq!(m.map("t1", q(13), Assoc::Before), Ok(q(14)));
}

#[test]
fn mirror_requires_equal_size() {
    let m = mapping(
        vec![step(&[("t1", 13, -3)]), step(&[("t1", 10, 2)])],
        &[(0, 1)],
    );
    assert_eq!(
        m.map("t1", q(11), Assoc::After),
        Err(Unmapped::Deleted { step: 0 })
    );
    let rec = Recover {
        shift: 0,
        offset: q(1),
    };
    let ins = step(&[("t1", 10, 2)]);
    assert_eq!(ins.recover(rec, q(3)).unwrap(), None);
    assert_eq!(ins.recover(rec, q(2)).unwrap(), Some(q(11)));
}

#[test]
fn mirror_requires_same_track() {
    let m = mapping(
        vec![step(&[("t1", 13, -3)]), step(&[("t2", 10, 3)])],
        &[(0, 1)],
    );
    assert_eq!(
        m.map("t1", q(11), Assoc::After),
        Err(Unmapped::Deleted { step: 0 })
    );
}

#[test]
fn mirror_pairs_shifts_in_reverse_order() {
    // A inserts 3 s at 10, then 2 s at 20; inverse(A) removes [20, 22) first, then [10, 13).
    let a = step(&[("t1", 10, 3), ("t1", 20, 2)]);
    let a2 = step(&[("t1", 15, 3), ("t1", 25, 2)]);
    let m = mapping(
        vec![a.invert().unwrap(), step(&[("t1", 0, 5)]), a2],
        &[(0, 2)],
    );
    assert_eq!(m.map("t1", q(21), Assoc::After), Ok(q(26)));
    assert_eq!(m.map("t1", q(11), Assoc::After), Ok(q(16)));
    assert_eq!(m.map("t1", q(30), Assoc::After), Ok(q(35)));
}

#[test]
fn mirror_is_used_only_forward() {
    // Steps 0 and 1 are mirrored, but a deletion at step 1 cannot recover through step 0.
    let m = mapping(
        vec![step(&[("t1", 0, 3)]), step(&[("t1", 13, -3)])],
        &[(0, 1)],
    );
    assert_eq!(
        m.map("t1", q(8), Assoc::After),
        Err(Unmapped::Deleted { step: 1 })
    );
}

#[test]
fn deletion_without_mirror_names_its_step() {
    let m = mapping(
        vec![
            step(&[("t2", 0, 4)]),
            step(&[("t1", 10, -3)]),
            step(&[("t1", 0, 1)]),
        ],
        &[],
    );
    assert_eq!(m.len(), 3);
    assert_eq!(
        m.map("t1", q(8), Assoc::After),
        Err(Unmapped::Deleted { step: 1 })
    );
    assert_eq!(m.map("t2", q(8), Assoc::After), Ok(q(12)));
}

// ---- Properties ----

proptest! {
    #![proptest_config(config())]

    #[test]
    fn empty_mapping_is_identity(pos in rational(-100, 100), a in assoc()) {
        prop_assert_eq!(Mapping::new().map("t1", pos, a), Ok(pos));
        let r = StepMap::default().map("t1", pos, a).unwrap();
        prop_assert_eq!((r.pos, r.deleted), (pos, None));
    }

    #[test]
    fn invert_is_involution(m in step_map(&["t1", "t2"])) {
        prop_assert_eq!(m.invert().unwrap().invert().unwrap(), m);
    }

    #[test]
    fn map_then_inverse_is_identity_outside_changed_ranges(
        m in step_map(&["t1", "t2"]),
        pos in rational(0, 150),
        a in assoc(),
    ) {
        let r = m.map("t1", pos, a).unwrap();
        prop_assume!(r.deleted.is_none());
        let back = m.invert().unwrap().map("t1", r.pos, a).unwrap();
        prop_assert_eq!((back.pos, back.deleted), (pos, None));
        let both = mapping(vec![m.clone(), m.invert().unwrap()], &[]);
        prop_assert_eq!(both.map("t1", pos, a), Ok(pos));
    }

    #[test]
    fn mirror_restores_positions_inside_inserts(
        at in rational(0, 60),
        size in rational(1, 20),
        frac in 0u8..8,
        between in step_map(&["t1", "t2"]),
        a in assoc(),
    ) {
        // A position `offset` into our own insert A, with After from its start edge up to
        // (not including) its end, resumes `offset` into the rebased A'.
        let offset = Rational::new(i64::from(frac) * size.num(), 8 * size.den()).unwrap();
        prop_assume!(a == Assoc::After || !offset.is_zero());
        let own = StepMap::from_shifts(vec![Shift { track: "t1".into(), from: at, by: size, op: 0 }]);
        let moved = between.map("t1", at, Assoc::After).unwrap();
        prop_assume!(moved.deleted.is_none());
        let rebased = StepMap::from_shifts(vec![Shift {
            track: "t1".into(),
            from: moved.pos,
            by: size,
            op: 0,
        }]);
        let m = mapping(vec![own.invert().unwrap(), between, rebased], &[(0, 2)]);
        let pos = at.checked_add(offset).unwrap();
        prop_assert_eq!(m.map("t1", pos, a), Ok(moved.pos.checked_add(offset).unwrap()));
    }

    #[test]
    fn after_assoc_matches_4a_shift_position(
        maps in prop::collection::vec(step_map(&["t1", "t2"]), 1..4),
        pos in rational(-10, 150),
    ) {
        // Equal to 4A unless strictly inside a removed range (edges included).
        let first = maps[0].map("t1", pos, Assoc::After).unwrap();
        if !first.across {
            prop_assert_eq!(first.pos, shift_position_4a("t1", pos, maps[0].shifts()));
        }
        let all: Vec<Shift> = maps.iter().flat_map(|m| m.shifts().to_vec()).collect();
        let joined = StepMap::from_shifts(all.clone()).map("t1", pos, Assoc::After).unwrap();
        let mapped = mapping(maps, &[]).map("t1", pos, Assoc::After);
        if joined.across {
            let is_deleted = matches!(mapped, Err(Unmapped::Deleted { .. }));
            prop_assert!(is_deleted);
        } else {
            prop_assert_eq!(mapped, Ok(shift_position_4a("t1", pos, &all)));
        }
    }

    #[test]
    fn mirrored_inverse_then_map_is_identity(
        a in step_map(&["t1", "t2"]),
        pos in rational(-10, 150),
        assoc in assoc(),
    ) {
        let m = mapping(vec![a.invert().unwrap(), a], &[(0, 1)]);
        prop_assert_eq!(m.map("t1", pos, assoc), Ok(pos));
    }

    #[test]
    fn other_tracks_are_untouched_by_any_map(
        m in step_map(&["t1"]),
        pos in rational(-10, 150),
        a in assoc(),
    ) {
        let r = m.map("t2", pos, a).unwrap();
        prop_assert_eq!((r.pos, r.deleted), (pos, None));
        prop_assert_eq!(mapping(vec![m.clone(), m], &[(0, 1)]).map("t2", pos, a), Ok(pos));
    }
}
